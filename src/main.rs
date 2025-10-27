mod errors;
mod utils;

use clap::{Parser, Subcommand};
use color_eyre::{Result, eyre::eyre};
use errors::FrorkError;
use mlua::prelude::*;
use serde::Deserialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::rc::Rc;
use tracing::{debug, error, info};
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

#[derive(Debug, Deserialize)]
struct Conflict {
    expected: String,
    actual: String,
}

impl FromLua for Conflict {
    fn from_lua(value: LuaValue, lua: &Lua) -> LuaResult<Self> {
        lua.from_value(value)
    }
}

#[derive(Debug)]
enum Status {
    Ok,
    Missing,
    ConflictUpgrade(Conflict),
}

impl FromLuaMulti for Status {
    fn from_lua_multi(values: LuaMultiValue, lua: &Lua) -> LuaResult<Self> {
        // Try two values: string and conflict for conflict-upgrade
        if let Ok((status_str, conflict)) =
            <(String, Conflict)>::from_lua_multi(values.clone(), lua)
        {
            return match status_str.as_str() {
                "conflict-upgrade" => Ok(Status::ConflictUpgrade(conflict)),
                _ => Err(LuaError::FromLuaConversionError {
                    from: "multivalue",
                    to: "Status".to_string(),
                    message: Some(
                        "String + conflict combination only supported for conflict-upgrade"
                            .to_string(),
                    ),
                }),
            };
        }

        // Try single string
        if let Ok(status_str) = String::from_lua_multi(values, lua) {
            return match status_str.as_str() {
                "ok" => Ok(Status::Ok),
                "missing" => Ok(Status::Missing),
                _ => Err(LuaError::FromLuaConversionError {
                    from: "string",
                    to: "Status".to_string(),
                    message: Some(format!("Invalid status string: '{}'", status_str)),
                }),
            };
        }

        Err(LuaError::FromLuaConversionError {
            from: "multivalue",
            to: "Status".to_string(),
            message: Some("Expected single string or conflict-upgrade with table".to_string()),
        })
    }
}

trait AssertionType: std::fmt::Display {
    fn status(&self) -> Result<Status>;
    fn install(&self) -> Result<()>;
}

#[derive(Clone)]
struct LuaAssertionType {
    display_fn: Option<LuaFunction>,
    status_fn: LuaFunction,
    install_fn: LuaFunction,
}

impl FromLua for LuaAssertionType {
    fn from_lua(value: LuaValue, lua: &Lua) -> LuaResult<Self> {
        let table = LuaTable::from_lua(value, lua)?;

        let display_fn: Option<LuaFunction> = table.get("display").ok();
        let status_fn: LuaFunction = table.get("status")?;
        let install_fn: LuaFunction = table.get("install")?;

        Ok(Self {
            display_fn,
            status_fn,
            install_fn,
        })
    }
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

struct Symlink {
    target: String,
    source: String,
}

impl FromLuaMulti for Symlink {
    fn from_lua_multi(args: LuaMultiValue, lua: &Lua) -> LuaResult<Self> {
        let (target, source) = <(String, String)>::from_lua_multi(args, lua)?;
        Ok(Self {
            target: Utils::expand_path(&target),
            source: Utils::expand_path(&source),
        })
    }
}

impl std::fmt::Display for Symlink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "symlink {} {}", self.target, self.source)
    }
}

impl AssertionType for Symlink {
    fn status(&self) -> Result<Status> {
        if !Path::new(&self.target).exists() {
            return Ok(Status::Missing);
        }

        if let Ok(link_target) = fs::read_link(&self.target) {
            if link_target == Path::new(&self.source) {
                Ok(Status::Ok)
            } else {
                todo!(
                    "{}",
                    format!(
                        "symlink {} {} points to wrong target",
                        self.target, self.source
                    )
                );
            }
        } else {
            todo!(
                "{}",
                format!(
                    "symlink {} {} target exists but is not a symlink",
                    self.target, self.source
                )
            );
        }
    }

    fn install(&self) -> Result<()> {
        use std::os::unix::fs;
        fs::symlink(&self.source, &self.target)
            .map_err(|e| eyre!("Failed to create symlink: {}", e))?;
        debug!("created: {}", self);
        Ok(())
    }
}

struct Directory {
    path: String,
}

impl FromLuaMulti for Directory {
    fn from_lua_multi(args: LuaMultiValue, lua: &Lua) -> LuaResult<Self> {
        let path = String::from_lua_multi(args, lua)?;
        Ok(Self {
            path: Utils::expand_path(&path),
        })
    }
}

impl std::fmt::Display for Directory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "directory {}", self.path)
    }
}

impl AssertionType for Directory {
    fn status(&self) -> Result<Status> {
        let path = Path::new(&self.path);
        if path.is_dir() {
            Ok(Status::Ok)
        } else if path.exists() {
            todo!(
                "{}",
                format!("directory {} exists but is not a directory", self.path)
            );
        } else {
            Ok(Status::Missing)
        }
    }

    fn install(&self) -> Result<()> {
        std::fs::create_dir_all(&self.path)
            .map_err(|e| eyre!("Failed to create directory: {}", e))?;
        debug!("created: {}", self);
        Ok(())
    }
}

struct Debug {
    status_fn: Option<LuaFunction>,
    install_fn: Option<LuaFunction>,
}

impl FromLuaMulti for Debug {
    fn from_lua_multi(args: LuaMultiValue, lua: &Lua) -> LuaResult<Self> {
        let table = LuaTable::from_lua_multi(args, lua)?;
        let status_fn: Option<LuaFunction> = table.get("status").ok();
        let install_fn: Option<LuaFunction> = table.get("install").ok();
        Ok(Self {
            status_fn,
            install_fn,
        })
    }
}

impl std::fmt::Display for Debug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "debug")
    }
}

impl AssertionType for Debug {
    fn status(&self) -> Result<Status> {
        if let Some(ref status_fn) = self.status_fn {
            let result = status_fn
                .call::<Status>(LuaMultiValue::new())
                .map_err(|e| eyre!("Debug status function failed: {}", e))?;
            Ok(result)
        } else {
            Ok(Status::Ok)
        }
    }

    fn install(&self) -> Result<()> {
        info!("debug: installing");
        let install_fn = self
            .install_fn
            .as_ref()
            .ok_or_else(|| eyre!("Install not implemented for debug assertion"))?;
        install_fn
            .call::<()>(LuaMultiValue::new())
            .map_err(|e| eyre!("Debug install function failed: {}", e))?;
        Ok(())
    }
}

struct LuaAssertion {
    name: String,
    args: LuaMultiValue,
    assertion_type: LuaAssertionType,
}

impl LuaAssertion {
    fn new(name: &str, args: LuaMultiValue, assertion_type: LuaAssertionType) -> Self {
        Self {
            name: name.to_string(),
            args,
            assertion_type,
        }
    }
}

impl std::fmt::Display for LuaAssertion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let default_display = || {
            let args_str = self
                .args
                .iter()
                .map(|v| v.to_string().unwrap_or_else(|_| "?".to_string()))
                .collect::<Vec<_>>()
                .join(" ");
            format!("{} {}", self.name, args_str)
        };

        if let Some(ref display_fn) = self.assertion_type.display_fn {
            let result = display_fn
                .call::<String>(self.args.clone())
                .unwrap_or_else(|err| {
                    error!("Display function failed for {}: {}", self.name, err);
                    default_display()
                });
            write!(f, "{}", result)
        } else {
            write!(f, "{}", default_display())
        }
    }
}

impl AssertionType for LuaAssertion {
    fn status(&self) -> Result<Status> {
        let result = self
            .assertion_type
            .status_fn
            .call::<Status>(self.args.clone())
            .map_err(FrorkError::from)?;
        Ok(result)
    }

    fn install(&self) -> Result<()> {
        self.assertion_type
            .install_fn
            .call::<()>(self.args.clone())
            .map_err(FrorkError::from)?;
        debug!("installed: {}", self);
        Ok(())
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
