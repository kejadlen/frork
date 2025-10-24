use anyhow::{Result, anyhow, bail};
use clap::{Parser, Subcommand};
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

#[derive(Error, Debug)]
pub enum FrorkError {
    #[error("No operation specified")]
    NoOperation,
    #[error("Unknown operation: {operation}")]
    UnknownOperation { operation: String },
    #[error("Symlink requires exactly 2 arguments: target and source")]
    InvalidSymlinkArgs,
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
    fn check(&self) -> Status;
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

    fn create(
        &self,
        assertion_type: &str,
        args: LuaMultiValue,
    ) -> Result<Option<Box<dyn AssertionType>>> {
        match self.assertion_types.get(assertion_type) {
            Some(factory) => factory(args).map(Some),
            None => Ok(None),
        }
    }
}

struct Symlink {
    target: String,
    source: String,
}

impl Symlink {
    fn new(args: LuaMultiValue) -> Result<Self> {
        let args_vec: Vec<LuaValue> = args.into_iter().collect();

        if args_vec.len() != 2 {
            bail!(FrorkError::InvalidSymlinkArgs);
        }

        let target = args_vec[0]
            .to_string()
            .map_err(|_| anyhow!("Target must be a string"))?;
        let source = args_vec[1]
            .to_string()
            .map_err(|_| anyhow!("Source must be a string"))?;

        Ok(Self {
            target: expand_tilde(&target),
            source: expand_tilde(&source),
        })
    }
}

impl AssertionType for Symlink {
    fn check(&self) -> Status {
        if Path::new(&self.target).exists() {
            // Check if it's a symlink pointing to the correct source
            if let Ok(link_target) = fs::read_link(&self.target) {
                if link_target == Path::new(&self.source) {
                    Status::Ok
                } else {
                    unimplemented!(
                        "symlink {} {} points to wrong target",
                        self.target,
                        self.source
                    );
                }
            } else {
                unimplemented!(
                    "symlink {} {} target exists but is not a symlink",
                    self.target,
                    self.source
                );
            }
        } else {
            Status::Missing
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
    fn check(&self) -> Status {
        // Call the Lua function with args and convert result
        match self.status_fn.call::<String>(self.args.clone()) {
            Ok(result) => match result.as_str() {
                "ok" => Status::Ok,
                "missing" => Status::Missing,
                _ => Status::Ok, // Default fallback
            },
            Err(_) => Status::Ok, // Default fallback on error
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
    registry: Rc<RefCell<Registry>>,
}

impl Frork {
    fn new() -> Self {
        let mut registry = Registry::new();
        registry.register("symlink", |args| Ok(Box::new(Symlink::new(args)?)));

        Self {
            registry: Rc::new(RefCell::new(registry)),
        }
    }

    fn ok(&self, args: LuaMultiValue) -> Result<()> {
        if args.is_empty() {
            bail!(FrorkError::NoOperation);
        }

        let mut args_iter = args.into_iter();
        let operation = args_iter
            .next()
            .and_then(|v| v.to_string().ok())
            .ok_or_else(|| anyhow!("First argument must be operation name"))?;

        let assertion_args: LuaMultiValue = args_iter.collect();

        match self.registry.borrow().create(&operation, assertion_args)? {
            Some(assertion) => {
                let status = assertion.check();
                match status {
                    Status::Ok => println!("ok: {}", assertion.display()),
                    Status::Missing => println!("missing: {}", assertion.display()),
                }
                Ok(())
            }
            None => bail!(FrorkError::UnknownOperation {
                operation: operation.to_string()
            }),
        }
    }

    fn register(&self, name: &str, table: LuaTable) -> Result<()> {
        let status_fn: LuaFunction = table
            .get("status")
            .map_err(|e| anyhow!("Failed to get status function: {}", e))?;

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

    fn bind(&self, lua: &Lua) -> LuaResult<LuaTable> {
        let frork_table = lua.create_table()?;
        let frork = Rc::new(Self::new());

        let frork_clone = frork.clone();
        let ok_fn = lua.create_function(move |_lua, args: LuaMultiValue| {
            frork_clone
                .ok(args)
                .map_err(|e| LuaError::RuntimeError(e.to_string()))
        })?;

        let frork_clone = frork.clone();
        let register_fn = lua.create_function(move |_lua, (name, table): (String, LuaTable)| {
            frork_clone
                .register(&name, table)
                .map_err(|e| LuaError::RuntimeError(e.to_string()))
        })?;

        frork_table.set("ok", ok_fn)?;
        frork_table.set("register", register_fn)?;
        Ok(frork_table)
    }
}

fn run(script_path: &str) -> Result<()> {
    let lua = Lua::new();

    let fennel_code = include_str!("../fennel-1.6.0.lua");
    let fennel_module = lua
        .load(fennel_code)
        .eval::<LuaValue>()
        .map_err(|e| anyhow!("Failed to load Fennel: {}", e))?;
    lua.register_module("fennel", fennel_module)
        .map_err(|e| anyhow!("Failed to register Fennel module: {}", e))?;

    let frork = Frork::new();
    let frork_module = frork
        .bind(&lua)
        .map_err(|e| anyhow!("Failed to create Frork module: {}", e))?;
    lua.register_module("frork", frork_module)
        .map_err(|e| anyhow!("Failed to register Frork module: {}", e))?;

    lua.load(format!(
        r#"require("fennel").install().dofile("{}")"#,
        script_path
    ))
    .exec()
    .map_err(|e| anyhow!("Failed to execute script '{}': {}", script_path, e))?;

    Ok(())
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match &cli.command {
        Commands::Check { script } => {
            run(script)?;
        }
    }

    Ok(())
}
