use clap::{Parser, Subcommand};
use color_eyre::{
    Result,
    eyre::{WrapErr, eyre},
};
use mlua::prelude::*;
use regex::Regex;
use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::rc::Rc;
use std::sync::LazyLock;
use thiserror::Error;
use tracing::{debug, info};

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
    #[error("Missing assertion type")]
    MissingAssertionType,
    #[error("Lua error: {0}")]
    Lua(String),
}

impl From<LuaError> for FrorkError {
    fn from(err: LuaError) -> Self {
        match err {
            LuaError::CallbackError { cause, .. } => {
                if let Some(frork_err) = cause.downcast_ref::<FrorkError>() {
                    frork_err.clone()
                } else {
                    FrorkError::Lua(format!("Callback error: {}", cause))
                }
            }
            _ => FrorkError::Lua(err.to_string()),
        }
    }
}

static ENV_VAR_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$([A-Za-z_][A-Za-z0-9_]*)").unwrap());

fn expand_path(path: &str) -> String {
    let mut expanded = path.to_string();

    // Expand tilde
    if expanded.starts_with('~') {
        if let Ok(home) = env::var("HOME") {
            expanded = expanded.replacen('~', &home, 1);
        }
    }

    // Expand environment variables using regex
    expanded = ENV_VAR_REGEX
        .replace_all(&expanded, |caps: &regex::Captures| {
            let var_name = caps.get(1).unwrap().as_str();
            env::var(var_name).unwrap_or_else(|_| caps.get(0).unwrap().as_str().to_string())
        })
        .to_string();

    expanded
}

#[derive(Debug)]
enum Status {
    Ok,
    Missing,
}

trait AssertionType: std::fmt::Display {
    fn status(&self) -> Result<Status>;
    fn install(&self) -> Result<()>;
}

type AssertionFactory = Box<dyn Fn(LuaMultiValue) -> Result<Box<dyn AssertionType>>>;

struct Registry {
    assertion_types: HashMap<String, AssertionFactory>,
}

impl Default for Registry {
    fn default() -> Self {
        let mut registry = Self {
            assertion_types: HashMap::new(),
        };

        registry.register("debug", |args| {
            Debug::new(args).map(|d| Box::new(d) as Box<dyn AssertionType>)
        });

        registry.register("directory", |args| {
            Directory::new(args).map(|d| Box::new(d) as Box<dyn AssertionType>)
        });
        registry.register("symlink", |args| {
            Symlink::new(args).map(|s| Box::new(s) as Box<dyn AssertionType>)
        });

        registry
    }
}

impl Registry {
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
                val.to_string().map(|s| expand_path(&s)).map_err(|_| {
                    FrorkError::InvalidArguments("Arguments must be strings".to_string())
                })
            })
            .collect::<Result<_, _>>()?;

        let [target, source]: [String; 2] = strings.try_into().unwrap();

        Ok(Self { target, source })
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

impl Directory {
    fn new(args: LuaMultiValue) -> Result<Self> {
        let args_vec: Vec<LuaValue> = args.into_vec();

        if args_vec.len() != 1 {
            return Err(FrorkError::InvalidArguments(format!(
                "Directory requires exactly 1 argument, got {}",
                args_vec.len()
            ))
            .into());
        }

        let path = args_vec[0]
            .to_string()
            .map_err(|_| FrorkError::InvalidArguments("Argument must be a string".to_string()))?;

        let expanded_path = expand_path(&path);

        Ok(Self {
            path: expanded_path,
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

impl Debug {
    fn new(args: LuaMultiValue) -> Result<Self> {
        let args_vec: Vec<LuaValue> = args.into_vec();

        if args_vec.len() != 1 {
            return Err(FrorkError::InvalidArguments(format!(
                "Debug requires exactly 1 argument, got {}",
                args_vec.len()
            ))
            .into());
        }

        let table = match &args_vec[0] {
            LuaValue::Table(t) => t.clone(),
            _ => {
                return Err(FrorkError::InvalidArguments(
                    "Debug argument must be a table".to_string(),
                )
                .into());
            }
        };

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
                .call::<String>(LuaMultiValue::new())
                .map_err(|e| eyre!("Debug status function failed: {}", e))?;
            match result.as_str() {
                "ok" => Ok(Status::Ok),
                "missing" => Ok(Status::Missing),
                _ => Err(eyre!("Invalid status returned: '{}'", result)),
            }
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
    display_fn: Option<LuaFunction>,
    status_fn: LuaFunction,
    install_fn: Option<LuaFunction>,
}

impl LuaAssertion {
    fn new(
        name: &str,
        args: LuaMultiValue,
        display_fn: Option<LuaFunction>,
        status_fn: LuaFunction,
        install_fn: LuaFunction,
    ) -> Self {
        Self {
            name: name.to_string(),
            args,
            display_fn,
            status_fn,
            install_fn: Some(install_fn),
        }
    }
}

impl std::fmt::Display for LuaAssertion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(ref display_fn) = self.display_fn {
            let result = display_fn
                .call::<String>(self.args.clone())
                .unwrap_or_else(|_| self.default_display());
            write!(f, "{}", result)
        } else {
            write!(f, "{}", self.default_display())
        }
    }
}

impl AssertionType for LuaAssertion {
    fn status(&self) -> Result<Status> {
        let result = self
            .status_fn
            .call::<String>(self.args.clone())
            .map_err(FrorkError::from)?;
        match result.as_str() {
            "ok" => Ok(Status::Ok),
            "missing" => Ok(Status::Missing),
            _ => Err(eyre!("Invalid status returned: '{}'", result)),
        }
    }

    fn install(&self) -> Result<()> {
        if let Some(ref install_fn) = self.install_fn {
            install_fn
                .call::<()>(self.args.clone())
                .map_err(FrorkError::from)?;
            debug!("installed: {}", self);
            Ok(())
        } else {
            Err(eyre!(
                "Install not implemented for assertion type: {}",
                self.name
            ))
        }
    }
}

impl LuaAssertion {
    fn default_display(&self) -> String {
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

impl Default for Frork {
    fn default() -> Self {
        Self {
            registry: RefCell::new(Registry::default()),
        }
    }
}

impl Frork {
    fn lua_table<F>(lua: &Lua, handle_status: F) -> Result<LuaTable>
    where
        F: Fn(&Status, &dyn AssertionType) -> Result<()> + Clone + 'static,
    {
        let frork_table = lua.create_table().map_err(FrorkError::from)?;
        let frork = Rc::new(Self::default());

        let frork_clone = frork.clone();
        let register_fn = lua
            .create_function(move |_lua, (name, table): (String, LuaTable)| {
                frork_clone.register(&name, table)
            })
            .map_err(FrorkError::from)?;

        frork_table
            .set("register", register_fn)
            .map_err(FrorkError::from)?;

        let frork_clone = frork.clone();
        let ok_fn = lua
            .create_function(move |_lua, args: LuaMultiValue| {
                frork_clone.ok(args, handle_status.clone())
            })
            .map_err(FrorkError::from)?;
        frork_table.set("ok", ok_fn).map_err(FrorkError::from)?;

        Ok(frork_table)
    }

    fn register(&self, name: &str, table: LuaTable) -> LuaResult<()> {
        let display_fn: Option<LuaFunction> = table.get("display").ok();
        let status_fn: LuaFunction = table.get("status")?;
        let install_fn: LuaFunction = table.get("install")?;

        let name_clone = name.to_string();
        self.registry.borrow_mut().register(name, move |args| {
            Ok(Box::new(LuaAssertion::new(
                &name_clone,
                args,
                display_fn.clone(),
                status_fn.clone(),
                install_fn.clone(),
            )))
        });
        info!("Registered assertion type: {}", name);
        Ok(())
    }

    fn ok<F>(&self, args: LuaMultiValue, handle_status: F) -> LuaResult<()>
    where
        F: Fn(&Status, &dyn AssertionType) -> Result<()>,
    {
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
            .create(&assertion_type, assertion_args)
            .map_err(LuaError::external)?;
        let status = assertion.status().map_err(LuaError::external)?;

        handle_status(&status, assertion.as_ref()).map_err(LuaError::external)?;
        Ok(())
    }
}

fn setup_fennel() -> Result<Lua> {
    let lua = Lua::new();

    let fennel_code = include_str!("../fennel-1.6.0.lua");
    let fennel_module = lua
        .load(fennel_code)
        .eval::<LuaValue>()
        .map_err(|e| eyre!("Failed to load Fennel: {}", e))?;
    lua.register_module("fennel", fennel_module)
        .map_err(|e| eyre!("Failed to register Fennel module: {}", e))?;

    Ok(lua)
}

fn run_code(
    code: &str,
    handle_status: impl Fn(&Status, &dyn AssertionType) -> Result<()> + Clone + 'static,
) -> Result<()> {
    let lua = setup_fennel()?;

    let frork = Rc::new(Frork::default());
    let frork_clone = frork.clone();
    let ok_fn = lua
        .create_function(move |_lua, args: LuaMultiValue| {
            frork_clone.ok(args, handle_status.clone())
        })
        .map_err(|e| eyre!("Failed to create ok function: {}", e))?;
    lua.globals()
        .set("ok", ok_fn)
        .map_err(|e| eyre!("Failed to set ok global: {}", e))?;

    let eval_fn: LuaFunction = lua
        .load("return require('fennel').eval")
        .eval()
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
    let lua = setup_fennel()?;

    let frork_module =
        Frork::lua_table(&lua, handle_status).wrap_err("Failed to create Frork module")?;
    lua.register_module("frork", &frork_module)
        .map_err(|e| eyre!("Failed to register Frork module: {}", e))?;

    lua.load(format!(
        r#"require("fennel").install().dofile("{}")"#,
        script
    ))
    .exec()
    .map_err(|e| eyre!("Failed to execute script: {}", e))?;

    Ok(())
}

fn run(command: &Commands) -> Result<()> {
    match command {
        Commands::Check { code } => run_code(code, |status, assertion| {
            match status {
                Status::Ok => println!("ok: {}", assertion),
                Status::Missing => println!("missing: {}", assertion),
            }
            Ok(())
        }),
        Commands::Do { code } => run_code(code, |status, assertion| {
            match status {
                Status::Ok => println!("ok: {}", assertion),
                Status::Missing => {
                    println!("missing: {}", assertion);
                    assertion.install()?;
                    println!("ok: {}", assertion);
                }
            }
            Ok(())
        }),
        Commands::Status { script } => run_script(script, |status, assertion| {
            match status {
                Status::Ok => println!("ok: {}", assertion),
                Status::Missing => println!("missing: {}", assertion),
            }
            Ok(())
        }),
        Commands::Satisfy { script } => run_script(script, |status, assertion| {
            match status {
                Status::Ok => println!("ok: {}", assertion),
                Status::Missing => {
                    println!("missing: {}", assertion);
                    assertion.install()?;
                    println!("ok: {}", assertion);
                }
            }
            Ok(())
        }),
    }
}

fn main() -> Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    run(&cli.command).wrap_err("Failed to run command")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lua_assertion_ok_status() {
        let lua = Lua::new();
        let status_fn = lua
            .create_function(|_lua, _args: LuaMultiValue| Ok("ok".to_string()))
            .unwrap();
        let install_fn = lua
            .create_function(|_lua, _args: LuaMultiValue| Ok(()))
            .unwrap();

        let assertion =
            LuaAssertion::new("test", LuaMultiValue::new(), None, status_fn, install_fn);
        let result = assertion.status().unwrap();

        match result {
            Status::Ok => {}
            _ => panic!("Expected Status::Ok"),
        }
    }

    #[test]
    fn test_lua_assertion_missing_status() {
        let lua = Lua::new();
        let status_fn = lua
            .create_function(|_lua, _args: LuaMultiValue| Ok("missing".to_string()))
            .unwrap();

        let install_fn = lua
            .create_function(|_lua, _args: LuaMultiValue| Ok(()))
            .unwrap();
        let assertion =
            LuaAssertion::new("test", LuaMultiValue::new(), None, status_fn, install_fn);
        let result = assertion.status().unwrap();

        match result {
            Status::Missing => {}
            _ => panic!("Expected Status::Missing"),
        }
    }

    #[test]
    fn test_lua_assertion_invalid_status() {
        let lua = Lua::new();
        let status_fn = lua
            .create_function(|_lua, _args: LuaMultiValue| Ok("invalid".to_string()))
            .unwrap();

        let install_fn = lua
            .create_function(|_lua, _args: LuaMultiValue| Ok(()))
            .unwrap();
        let assertion =
            LuaAssertion::new("test", LuaMultiValue::new(), None, status_fn, install_fn);
        let result = assertion.status();

        assert!(result.is_err());
    }

    #[test]
    fn test_lua_assertion_display() {
        let lua = Lua::new();
        let status_fn = lua
            .create_function(|_lua, _args: LuaMultiValue| Ok("ok".to_string()))
            .unwrap();

        let args = vec![LuaValue::String(lua.create_string("arg1").unwrap())];
        let lua_args = LuaMultiValue::from_vec(args);

        let install_fn = lua
            .create_function(|_lua, _args: LuaMultiValue| Ok(()))
            .unwrap();
        let assertion = LuaAssertion::new("mytest", lua_args, None, status_fn, install_fn);
        let display = assertion.to_string();

        assert!(display.contains("mytest"));
        assert!(display.contains("arg1"));
    }

    #[test]
    fn test_lua_assertion_custom_display() {
        let lua = Lua::new();
        let status_fn = lua
            .create_function(|_lua, _args: LuaMultiValue| Ok("ok".to_string()))
            .unwrap();
        let display_fn = lua
            .create_function(|_lua, args: LuaMultiValue| {
                let arg_str = args
                    .into_iter()
                    .map(|v| v.to_string().unwrap_or_else(|_| "?".to_string()))
                    .collect::<Vec<_>>()
                    .join(",");
                Ok(format!("custom: {}", arg_str))
            })
            .unwrap();

        let args = vec![LuaValue::String(lua.create_string("test1").unwrap())];
        let lua_args = LuaMultiValue::from_vec(args);

        let install_fn = lua
            .create_function(|_lua, _args: LuaMultiValue| Ok(()))
            .unwrap();
        let assertion =
            LuaAssertion::new("mytest", lua_args, Some(display_fn), status_fn, install_fn);
        let display = assertion.to_string();

        assert_eq!(display, "custom: test1");
    }

    #[test]
    fn test_fennel_assertion_registration() {
        let lua = Lua::new();

        let fennel_code = include_str!("../fennel-1.6.0.lua");
        let fennel_module = lua.load(fennel_code).eval::<LuaValue>().unwrap();
        lua.globals().set("fennel", fennel_module).unwrap();

        let fennel: LuaTable = lua.globals().get("fennel").unwrap();
        let eval_fn: LuaFunction = fennel.get("eval").unwrap();

        let assertion_table: LuaTable = eval_fn
            .call(
                r#"
            {:display (fn [args] (.. "test: " (or (. args 1) "default")))
             :status (fn [args] :ok)
             :install (fn [args] nil)}
        "#,
            )
            .unwrap();

        let frork = Frork::default();
        frork.register("test-assertion", assertion_table).unwrap();
    }
}
