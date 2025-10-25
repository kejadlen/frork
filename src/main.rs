use clap::{Parser, Subcommand};
use color_eyre::{
    Result,
    eyre::{WrapErr, eyre},
};
use mlua::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::rc::Rc;
use thiserror::Error;
use tracing::info;

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

#[derive(Error, Debug, Clone)]
pub enum FrorkError {
    #[error("No operation specified")]
    NoOperation,
    #[error("Unknown operation: {operation}")]
    UnknownOperation { operation: String },
    #[error("Unknown assertion type: {assertion_type}")]
    UnknownAssertionType { assertion_type: String },
    #[error("Invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("Lua error: {0}")]
    Lua(String),
    #[error("Assertion error: {0}")]
    Assertion(String),
}

impl From<LuaError> for FrorkError {
    fn from(err: LuaError) -> Self {
        FrorkError::Lua(err.to_string())
    }
}

fn expand_tilde(path: &str) -> String {
    if path.starts_with('~') {
        if let Ok(home) = env::var("HOME") {
            path.replacen('~', &home, 1)
        } else {
            path.to_string()
        }
    } else {
        path.to_string()
    }
}

#[derive(Debug)]
enum Status {
    Ok,
    Missing,
}

trait AssertionType {
    fn status(&self) -> Result<Status>;
    fn display(&self) -> String;
}

type AssertionFactory = Box<dyn Fn(LuaMultiValue) -> Result<Box<dyn AssertionType>>>;

struct Registry {
    assertion_types: HashMap<String, AssertionFactory>,
}

impl Registry {
    fn new() -> Self {
        Self {
            assertion_types: HashMap::new(),
        }
    }

    fn register<F>(&mut self, name: &str, factory: F)
    where
        F: Fn(LuaMultiValue) -> Result<Box<dyn AssertionType>> + 'static,
    {
        self.assertion_types
            .insert(name.to_string(), Box::new(factory));
    }

    fn create(&self, assertion_type: &str, args: LuaMultiValue) -> Result<Box<dyn AssertionType>> {
        let factory = self.assertion_types.get(assertion_type).ok_or_else(|| {
            FrorkError::UnknownAssertionType {
                assertion_type: assertion_type.to_string(),
            }
        })?;
        factory(args)
    }
}

struct Symlink {
    target: String,
    source: String,
}

impl Symlink {
    fn new(args: LuaMultiValue) -> Result<Self> {
        let args_vec: Vec<LuaValue> = args.into_vec();

        if args_vec.len() != 2 {
            return Err(FrorkError::InvalidArguments(format!(
                "Symlink requires exactly 2 arguments, got {}",
                args_vec.len()
            ))
            .into());
        }

        let strings: Vec<String> = args_vec
            .into_iter()
            .map(|val| {
                val.to_string().map(|s| expand_tilde(&s)).map_err(|_| {
                    FrorkError::InvalidArguments("Arguments must be strings".to_string())
                })
            })
            .collect::<Result<_, _>>()?;

        let [target, source]: [String; 2] = strings.try_into().unwrap();

        Ok(Self { target, source })
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
                unimplemented!(
                    "{}",
                    FrorkError::Assertion(format!(
                        "symlink {} {} points to wrong target",
                        self.target, self.source
                    ))
                );
            }
        } else {
            unimplemented!(
                "{}",
                FrorkError::Assertion(format!(
                    "symlink {} {} target exists but is not a symlink",
                    self.target, self.source
                ))
            );
        }
    }

    fn display(&self) -> String {
        format!("symlink {} {}", self.target, self.source)
    }
}

struct FennelAssertion {
    name: String,
    args: LuaMultiValue,
    status_fn: LuaFunction,
}

impl FennelAssertion {
    fn new(name: &str, args: LuaMultiValue, status_fn: LuaFunction) -> Self {
        Self {
            name: name.to_string(),
            args,
            status_fn,
        }
    }
}

impl AssertionType for FennelAssertion {
    fn status(&self) -> Result<Status> {
        let result = self
            .status_fn
            .call::<String>(self.args.clone())
            .map_err(FrorkError::from)?;
        match result.as_str() {
            "ok" => Ok(Status::Ok),
            "missing" => Ok(Status::Missing),
            _ => {
                Err(FrorkError::Assertion(format!("Invalid status returned: '{}'", result)).into())
            }
        }
    }

    fn display(&self) -> String {
        let args_str = self
            .args
            .iter()
            .map(|v| v.to_string().unwrap_or_else(|_| "?".to_string()))
            .collect::<Vec<_>>()
            .join(" ");
        format!("{} {}", self.name, args_str)
    }
}

struct Frork {
    // RefCell needed for interior mutability - register() method needs to add
    // new assertion types at runtime when called from Lua/Fennel code
    registry: RefCell<Registry>,
}

impl Frork {
    fn new() -> Self {
        let mut registry = Registry::new();
        registry.register("symlink", |args| {
            Symlink::new(args).map(|s| Box::new(s) as Box<dyn AssertionType>)
        });

        Self {
            registry: RefCell::new(registry),
        }
    }

    fn check(&self, args: LuaMultiValue) -> Result<()> {
        if args.is_empty() {
            return Err(FrorkError::NoOperation.into());
        }

        let mut args_iter = args.into_iter();
        let assertion_type = args_iter
            .next()
            .and_then(|v| v.to_string().ok())
            .ok_or_else(|| {
                FrorkError::InvalidArguments("First argument must be assertion type".to_string())
            })?;

        let assertion_args: LuaMultiValue = args_iter.collect();

        let assertion = self
            .registry
            .borrow()
            .create(&assertion_type, assertion_args)?;
        let status = assertion.status()?;
        match status {
            Status::Ok => println!("ok: {}", assertion.display()),
            Status::Missing => println!("missing: {}", assertion.display()),
        }
        Ok(())
    }

    fn register(&self, name: &str, table: LuaTable) -> Result<()> {
        let status_fn: LuaFunction = table.get("status").map_err(FrorkError::from)?;

        let name_clone = name.to_string();
        self.registry.borrow_mut().register(name, move |args| {
            Ok(Box::new(FennelAssertion::new(
                &name_clone,
                args,
                status_fn.clone(),
            )))
        });
        info!("Registered assertion type: {}", name);
        Ok(())
    }

    fn lua_table(lua: &Lua) -> Result<LuaTable> {
        let frork_table = lua.create_table().map_err(FrorkError::from)?;
        let frork = Rc::new(Self::new());

        let frork_clone = frork.clone();
        let check_fn = lua
            .create_function(move |_lua, args: LuaMultiValue| {
                frork_clone.check(args).map_err(LuaError::external)
            })
            .map_err(|e| match e {
                LuaError::CallbackError { cause, .. } => {
                    if let Some(frork_err) = cause.downcast_ref::<FrorkError>() {
                        frork_err.clone().into()
                    } else {
                        eyre!("Callback error: {}", cause)
                    }
                }
                _ => FrorkError::Lua(e.to_string()).into(),
            })?;

        let frork_clone = frork.clone();
        let register_fn = lua
            .create_function(move |_lua, (name, table): (String, LuaTable)| {
                frork_clone
                    .register(&name, table)
                    .map_err(LuaError::external)
            })
            .map_err(|e| match e {
                LuaError::CallbackError { cause, .. } => {
                    if let Some(frork_err) = cause.downcast_ref::<FrorkError>() {
                        frork_err.clone().into()
                    } else {
                        eyre!("Callback error: {}", cause)
                    }
                }
                _ => FrorkError::Lua(e.to_string()).into(),
            })?;

        frork_table.set("ok", check_fn).map_err(FrorkError::from)?;
        frork_table
            .set("register", register_fn)
            .map_err(FrorkError::from)?;
        Ok(frork_table)
    }
}

fn run(command: &Commands) -> color_eyre::Result<()> {
    match command {
        Commands::Check { script } => {
            let lua = Lua::new();

            let fennel_code = include_str!("../fennel-1.6.0.lua");
            let fennel_module = lua
                .load(fennel_code)
                .eval::<LuaValue>()
                .map_err(|e| eyre!("Failed to load Fennel: {}", e))?;
            lua.register_module("fennel", fennel_module)
                .map_err(|e| eyre!("Failed to register Fennel module: {}", e))?;

            let frork_module = Frork::lua_table(&lua)
                .map_err(|e| eyre!("Failed to create Frork module: {}", e))?;
            lua.register_module("frork", frork_module)
                .map_err(|e| eyre!("Failed to register Frork module: {}", e))?;

            lua.load(format!(
                r#"require("fennel").install().dofile("{}")"#,
                script
            ))
            .exec()
            .map_err(|e| eyre!("Failed to execute script: {}", e))?;
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    run(&cli.command).wrap_err("Failed to run command")?;

    Ok(())
}
