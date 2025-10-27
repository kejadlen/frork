use clap::{Parser, Subcommand};
use color_eyre::{Result, eyre::eyre};
use mlua::prelude::*;
use regex::Regex;
use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::sync::LazyLock;
use thiserror::Error;
use tracing::{debug, error, info};

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

// TODO does this actually work the way I think it does? write a test to find out
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

#[derive(Debug)]
enum Status {
    Ok,
    Missing,
}

trait AssertionType: std::fmt::Display {
    fn status(&self) -> Result<Status>;
    fn install(&self) -> Result<()>;
}

struct LuaAssertionType {
    display_fn: Option<LuaFunction>,
    status_fn: LuaFunction,
    install_fn: LuaFunction,
}

impl FromLua for LuaAssertionType {
    fn from_lua(value: LuaValue, _lua: &Lua) -> LuaResult<Self> {
        let table = match value {
            LuaValue::Table(table) => table,
            _ => {
                return Err(LuaError::FromLuaConversionError {
                    from: value.type_name(),
                    to: "LuaAssertionType".to_string(),
                    message: Some("Expected a table".to_string()),
                });
            }
        };

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

    fn create(&self, assertion_type: &str, args: LuaMultiValue) -> Result<Box<dyn AssertionType>> {
        // Check Lua assertions first
        if let Some(lua_assertion) = self.lua_assertion_types.get(assertion_type) {
            return Ok(Box::new(LuaAssertion::new(
                assertion_type,
                args,
                lua_assertion.display_fn.clone(),
                lua_assertion.status_fn.clone(),
                lua_assertion.install_fn.clone(),
            )));
        }

        // Fall back to built-in types
        match assertion_type {
            "debug" => Debug::new(args).map(|d| Box::new(d) as Box<dyn AssertionType>),
            "directory" => Directory::new(args).map(|d| Box::new(d) as Box<dyn AssertionType>),
            "symlink" => Symlink::new(args).map(|s| Box::new(s) as Box<dyn AssertionType>),
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

impl Symlink {
    fn new(args: LuaMultiValue) -> Result<Self> {
        let (target, source) =
            <(String, String)>::from_lua_multi(args, &Lua::new()).map_err(|_| {
                FrorkError::InvalidArguments(
                    "Symlink requires exactly 2 string arguments".to_string(),
                )
            })?;

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

impl Directory {
    fn new(args: LuaMultiValue) -> Result<Self> {
        let path = String::from_lua_multi(args, &Lua::new()).map_err(|_| {
            FrorkError::InvalidArguments("Directory requires exactly 1 string argument".to_string())
        })?;

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

impl Debug {
    fn new(args: LuaMultiValue) -> Result<Self> {
        let table = LuaTable::from_lua_multi(args, &Lua::new()).map_err(|_| {
            FrorkError::InvalidArguments("Debug requires exactly 1 table argument".to_string())
        })?;

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
    install_fn: LuaFunction,
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
            install_fn,
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

        if let Some(ref display_fn) = self.display_fn {
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
        self.install_fn
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
}

impl<F> Frork<F> {
    fn new(handle_status: F) -> Self {
        Self {
            registry: RefCell::new(Registry::default()),
            handle_status,
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
            .create(&assertion_type, assertion_args)
            .map_err(LuaError::external)?;
        let status = assertion.status().map_err(LuaError::external)?;

        (self.handle_status)(&status, assertion.as_ref()).map_err(LuaError::external)?;
        Ok(())
    }
}

struct Utils;

impl Utils {
    fn chomp(s: &str) -> String {
        s.trim_end_matches('\n').trim_end_matches('\r').to_string()
    }

    fn dirname(path: &str) -> Option<String> {
        let expanded_path = Self::expand_path(path);
        let parent = Path::new(&expanded_path).parent()?;
        parent.to_str().map(|s| s.to_string())
    }

    fn expand_path(path: &str) -> String {
        static ENV_VAR_REGEX: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"\$([A-Za-z_][A-Za-z0-9_]*)").unwrap());

        let mut expanded = path.to_string();

        // Expand tilde
        if expanded.starts_with('~')
            && let Ok(home) = env::var("HOME")
        {
            expanded = expanded.replacen('~', &home, 1);
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

    fn sh(cmd: &str, args: &[String]) -> Result<(String, i32)> {
        debug!("Executing command: {} with args: {:?}", cmd, args);

        let output = Command::new(cmd)
            .args(args)
            .output()
            .map_err(|e| eyre!("Failed to execute command '{}': {}", cmd, e))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let status = output
            .status
            .code()
            .ok_or_else(|| eyre!("Command '{}' terminated by signal", cmd))?;
        debug!(
            "Command '{}' completed with status {}, stdout: {:?}",
            cmd, status, stdout
        );
        Ok((stdout, status))
    }
}

impl IntoLua for Utils {
    fn into_lua(self, lua: &Lua) -> LuaResult<LuaValue> {
        let utils_table = lua.create_table()?;

        utils_table.set(
            "expand_path",
            lua.create_function(|_lua, path: String| Ok(Utils::expand_path(&path)))?,
        )?;
        utils_table.set(
            "dirname",
            lua.create_function(|_lua, path: String| Ok(Utils::dirname(&path)))?,
        )?;
        utils_table.set(
            "chomp",
            lua.create_function(|_lua, s: String| Ok(Utils::chomp(&s)))?,
        )?;
        utils_table.set(
            "sh",
            lua.create_function(|lua, args: LuaMultiValue| {
                let all_args: Vec<String> = args
                    .into_iter()
                    .map(|arg| String::from_lua(arg, lua))
                    .collect::<Result<Vec<_>, _>>()?;

                if all_args.is_empty() {
                    return Err(LuaError::external(FrorkError::InvalidArguments(
                        "sh requires at least a command".to_string(),
                    )));
                }

                let (cmd, cmd_args) = all_args.split_first().unwrap();

                let result = Utils::sh(cmd, cmd_args)
                    .map(|(stdout, status)| (Some(stdout), status))
                    .unwrap_or((None, -1));
                Ok(result)
            })?,
        )?;

        Ok(LuaValue::Table(utils_table))
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

    let frork_table = match Frork::new(handle_status)
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
