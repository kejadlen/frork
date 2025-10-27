mod assertions;
mod errors;
mod utils;

use assertions::{
    AssertionType, Debug, Directory, LuaAssertion, LuaAssertionType, Status, Symlink,
};
use clap::{Parser, Subcommand};
use color_eyre::{Result, eyre::eyre};
use errors::FrorkError;
use mlua::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use tracing::info;
use utils::Utils;

#[derive(Parser)]
#[command(name = "frork")]
#[command(about = "A Fennel-based configuration management tool")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Check { code: String },
    Do { code: String },
    Status { script: String },
    Satisfy { script: String },
}

#[derive(Default)]
struct Registry {
    lua_assertion_types: HashMap<String, LuaAssertionType>,
}

impl Registry {
    fn register(&mut self, name: &str, lua_assertion_type: LuaAssertionType) {
        self.lua_assertion_types
            .insert(name.to_string(), lua_assertion_type);
    }

    fn create(
        &self,
        assertion_type: &str,
        args: LuaMultiValue,
        lua: &Lua,
    ) -> Result<Box<dyn AssertionType>> {
        // Check Lua assertions first
        if let Some(lua_assertion) = self.lua_assertion_types.get(assertion_type) {
            return Ok(Box::new(LuaAssertion::new(
                assertion_type,
                args,
                lua_assertion.clone(),
            )));
        }

        // Fall back to built-in types
        match assertion_type {
            "debug" => Debug::from_lua_multi(args, lua)
                .map(|d| Box::new(d) as Box<dyn AssertionType>)
                .map_err(|e| FrorkError::from(e).into()),
            "directory" => Directory::from_lua_multi(args, lua)
                .map(|d| Box::new(d) as Box<dyn AssertionType>)
                .map_err(|e| FrorkError::from(e).into()),
            "symlink" => Symlink::from_lua_multi(args, lua)
                .map(|s| Box::new(s) as Box<dyn AssertionType>)
                .map_err(|e| FrorkError::from(e).into()),
            _ => Err(FrorkError::UnknownAssertionType {
                assertion_type: assertion_type.to_string(),
            }
            .into()),
        }
    }
}

struct Frork<F> {
    // RefCell needed for interior mutability - register() method needs to add
    // new assertion types at runtime when called from Lua/Fennel code
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

impl<F> IntoLua for Frork<F>
where
    F: Fn(&Status, &dyn AssertionType) -> Result<()> + Clone + 'static,
{
    fn into_lua(self, lua: &Lua) -> LuaResult<LuaValue> {
        let frork_table = lua.create_table()?;
        let frork = Rc::new(self);

        let frork_clone = frork.clone();
        let register_fn = lua.create_function(
            move |_lua, (name, lua_assertion_type): (String, LuaAssertionType)| {
                frork_clone.register(&name, lua_assertion_type)
            },
        )?;

        frork_table.set("register", register_fn)?;

        let frork_clone = frork.clone();
        let ok_fn = lua.create_function(move |_lua, args: LuaMultiValue| frork_clone.ok(args))?;
        frork_table.set("ok", ok_fn)?;

        frork_table.set("utils", Utils {})?;

        Ok(LuaValue::Table(frork_table))
    }
}

impl<F> Frork<F>
where
    F: Fn(&Status, &dyn AssertionType) -> Result<()> + Clone + 'static,
{
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

        let assertion = self
            .registry
            .borrow()
            .create(&assertion_type, assertion_args, &self.lua)
            .map_err(LuaError::external)?;
        let status = assertion.status().map_err(LuaError::external)?;

        (self.handle_status)(&status, assertion.as_ref()).map_err(LuaError::external)?;
        Ok(())
    }
}

fn setup_lua(
    handle_status: impl Fn(&Status, &dyn AssertionType) -> Result<()> + Clone + 'static,
) -> Result<(Lua, LuaTable, LuaTable)> {
    let lua = Lua::new();

    let fennel_code = include_str!("../fennel-1.6.0.lua");
    let fennel_module: LuaTable = lua
        .load(fennel_code)
        .eval()
        .map_err(|e| eyre!("Failed to load Fennel: {}", e))?;
    lua.register_module("fennel", &fennel_module)
        .map_err(|e| eyre!("Failed to register Fennel module: {}", e))?;

    let frork_table = match Frork::new(handle_status, lua.clone())
        .into_lua(&lua)
        .map_err(|e| eyre!("Failed to create Frork table: {}", e))?
    {
        LuaValue::Table(table) => table,
        _ => unreachable!(),
    };
    lua.register_module("frork", &frork_table)
        .map_err(|e| eyre!("Failed to register Frork module: {}", e))?;
    lua.register_module("flork", &frork_table)
        .map_err(|e| eyre!("Failed to register Flork module: {}", e))?;

    Ok((lua, frork_table, fennel_module))
}

fn run_code(
    code: &str,
    handle_status: impl Fn(&Status, &dyn AssertionType) -> Result<()> + Clone + 'static,
) -> Result<()> {
    let (lua, frork_module, fennel_module) = setup_lua(handle_status)?;

    let ok_fn: LuaFunction = frork_module
        .get("ok")
        .map_err(|e| eyre!("Failed to get frork.ok: {}", e))?;
    lua.globals()
        .set("ok", ok_fn)
        .map_err(|e| eyre!("Failed to set ok global: {}", e))?;

    let eval_fn: LuaFunction = fennel_module
        .get("eval")
        .map_err(|e| eyre!("Failed to get fennel.eval: {}", e))?;

    eval_fn
        .call::<()>(code)
        .map_err(|e| eyre!("Failed to execute fennel code: {}", e))?;

    Ok(())
}

fn run_script(
    script: &str,
    handle_status: impl Fn(&Status, &dyn AssertionType) -> Result<()> + Clone + 'static,
) -> Result<()> {
    let (lua, _frork_module, _fennel_module) = setup_lua(handle_status)?;

    lua.load(format!(
        r#"require("fennel").install().dofile("{}")"#,
        script
    ))
    .exec()
    .map_err(|e| eyre!("Failed to execute script: {}", e))?;

    Ok(())
}

fn status(status: &Status, assertion: &dyn AssertionType) -> Result<()> {
    match status {
        Status::Ok => println!("ok: {}", assertion),
        Status::Missing => println!("missing: {}", assertion),
        Status::ConflictUpgrade(conflict) => {
            println!("conflict (upgradable): {}", assertion);
            println!("  expected: {}", conflict.expected);
            println!("    actual: {}", conflict.actual);
        }
    }
    Ok(())
}

fn satisfy(status: &Status, assertion: &dyn AssertionType) -> Result<()> {
    match status {
        Status::Ok => println!("ok: {}", assertion),
        Status::Missing => {
            println!("missing: {}", assertion);
            assertion.install()?;
            println!("ok: {}", assertion);
        }
        Status::ConflictUpgrade(conflict) => {
            todo!(
                "Handle conflict upgrade status in satisfy: expected: {}, actual: {}",
                conflict.expected,
                conflict.actual
            );
        }
    }
    Ok(())
}

fn run(command: &Commands) -> Result<()> {
    match command {
        Commands::Check { code } => run_code(code, status),
        Commands::Do { code } => run_code(code, satisfy),
        Commands::Status { script } => run_script(script, status),
        Commands::Satisfy { script } => run_script(script, satisfy),
    }
}

fn main() -> Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    run(&cli.command)?;

    Ok(())
}
