use color_eyre::{Result, eyre::eyre};
use mlua::prelude::*;
use serde::Deserialize;
use std::fs;
use std::path::Path;
use tracing::{debug, error, info};

use crate::errors::FrorkError;
use crate::utils::{ExpandedPath, Utils};

pub trait AssertionTypeFactory {
    fn create(&self, lua: &Lua, args: LuaMultiValue) -> Result<Box<dyn AssertionType>>;
}

pub struct TypedFactory<T>(std::marker::PhantomData<T>);

impl<T> TypedFactory<T> {
    pub fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<T> Default for TypedFactory<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> AssertionTypeFactory for TypedFactory<T>
where
    T: AssertionType + FromLuaMulti + 'static,
{
    fn create(&self, lua: &Lua, args: LuaMultiValue) -> Result<Box<dyn AssertionType>> {
        T::from_lua_multi(args, lua)
            .map(|t| Box::new(t) as Box<dyn AssertionType>)
            .map_err(|e| eyre!("Failed to create assertion type: {}", e))
    }
}

#[derive(Debug, Deserialize)]
pub struct Conflict {
    pub expected: String,
    pub actual: String,
}

impl FromLua for Conflict {
    fn from_lua(value: LuaValue, lua: &Lua) -> LuaResult<Self> {
        lua.from_value(value)
    }
}

#[derive(Debug)]
pub enum Status {
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

pub trait AssertionType: std::fmt::Display {
    fn status(&self) -> Result<Status>;
    fn install(&self) -> Result<()>;
    fn upgrade(&self) -> Result<()>;
    fn remove(&self) -> Result<()>;
}

#[derive(Clone)]
pub struct LuaAssertionType {
    pub display_fn: Option<LuaFunction>,
    pub status_fn: LuaFunction,
    pub install_fn: LuaFunction,
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

pub struct Symlink {
    pub target: ExpandedPath,
    pub source: ExpandedPath,
}

impl FromLuaMulti for Symlink {
    fn from_lua_multi(args: LuaMultiValue, lua: &Lua) -> LuaResult<Self> {
        let (target, source) = <(ExpandedPath, ExpandedPath)>::from_lua_multi(args, lua)?;
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

    fn upgrade(&self) -> Result<()> {
        todo!()
    }

    fn remove(&self) -> Result<()> {
        todo!()
    }
}

pub struct Directory {
    pub path: ExpandedPath,
}

impl FromLuaMulti for Directory {
    fn from_lua_multi(args: LuaMultiValue, lua: &Lua) -> LuaResult<Self> {
        let path = ExpandedPath::from_lua_multi(args, lua)?;
        Ok(Self { path })
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

    fn upgrade(&self) -> Result<()> {
        todo!()
    }

    fn remove(&self) -> Result<()> {
        todo!()
    }
}

pub struct Debug {
    pub status_fn: Option<LuaFunction>,
    pub install_fn: Option<LuaFunction>,
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

    fn upgrade(&self) -> Result<()> {
        todo!()
    }

    fn remove(&self) -> Result<()> {
        todo!()
    }
}

pub struct Git {
    pub dir: ExpandedPath,
    pub remote_url: String,
}

impl FromLuaMulti for Git {
    fn from_lua_multi(args: LuaMultiValue, lua: &Lua) -> LuaResult<Self> {
        let (dir, remote_url) = <(ExpandedPath, String)>::from_lua_multi(args, lua)?;
        Ok(Self { dir, remote_url })
    }
}

impl std::fmt::Display for Git {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "git {} {}", self.dir, self.remote_url)
    }
}

impl AssertionType for Git {
    fn status(&self) -> Result<Status> {
        // First, ensure git binary is available
        Utils::assert_bin("git")?;

        let dir_path = Path::new(&self.dir);

        // Check if directory exists
        if !dir_path.exists() {
            return Ok(Status::Missing);
        }

        // Check if it's a directory
        if !dir_path.is_dir() {
            return Ok(Status::ConflictUpgrade(Conflict {
                expected: "directory".to_string(),
                actual: "file".to_string(),
            }));
        }

        // Check if directory is empty (only . and ..)
        let mut entries =
            std::fs::read_dir(dir_path).map_err(|e| eyre!("Failed to read directory: {}", e))?;
        if entries.next().is_none() {
            return Ok(Status::Missing);
        }

        // Check if we can get the git remote
        let (remote_output, exit_code) = Utils::sh(
            "git",
            &[
                "-C".to_string(),
                self.dir.to_string(),
                "config".to_string(),
                "--get".to_string(),
                "remote.origin.url".to_string(),
            ],
        )?;

        if exit_code != 0 {
            return Ok(Status::ConflictUpgrade(Conflict {
                expected: format!("git repo with remote {}", self.remote_url),
                actual: "failed to get remote url".to_string(),
            }));
        }

        let current_remote = Utils::chomp(&remote_output);
        if current_remote == self.remote_url {
            Ok(Status::Ok)
        } else {
            Ok(Status::ConflictUpgrade(Conflict {
                expected: self.remote_url.clone(),
                actual: format!("current remote: {}", current_remote),
            }))
        }
    }

    fn install(&self) -> Result<()> {
        // Ensure git binary is available before attempting clone
        Utils::assert_bin("git")?;

        let (_output, exit_code) = Utils::sh(
            "git",
            &[
                "clone".to_string(),
                self.remote_url.clone(),
                self.dir.to_string(),
            ],
        )?;

        if exit_code != 0 {
            return Err(eyre!("Failed to clone git repository"));
        }

        debug!("created: {}", self);
        Ok(())
    }

    fn upgrade(&self) -> Result<()> {
        todo!()
    }

    fn remove(&self) -> Result<()> {
        todo!()
    }
}

pub struct LuaAssertion {
    pub name: String,
    pub args: LuaMultiValue,
    pub assertion_type: LuaAssertionType,
}

impl LuaAssertion {
    pub fn new(name: &str, args: LuaMultiValue, assertion_type: LuaAssertionType) -> Self {
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

    fn upgrade(&self) -> Result<()> {
        todo!()
    }

    fn remove(&self) -> Result<()> {
        todo!()
    }
}

pub struct Brew;

impl FromLuaMulti for Brew {
    fn from_lua_multi(_args: LuaMultiValue, _lua: &Lua) -> LuaResult<Self> {
        Ok(Self)
    }
}

impl std::fmt::Display for Brew {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "brew")
    }
}

impl AssertionType for Brew {
    fn status(&self) -> Result<Status> {
        let (_output, exit_code) = Utils::sh("brew", &["--version".to_string()])?;
        if exit_code == 0 {
            Ok(Status::Ok)
        } else {
            Ok(Status::Missing)
        }
    }

    fn install(&self) -> Result<()> {
        let (_output, exit_code) = Utils::sh(
            "bash",
            &[
                "-c".to_string(),
                r#"/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)""#.to_string(),
            ],
        )?;

        if exit_code != 0 {
            return Err(eyre!("Failed to install Homebrew"));
        }

        debug!("installed: {}", self);
        Ok(())
    }

    fn upgrade(&self) -> Result<()> {
        todo!()
    }

    fn remove(&self) -> Result<()> {
        todo!()
    }
}

pub struct BrewBundle {
    pub brewfile: ExpandedPath,
}

impl FromLuaMulti for BrewBundle {
    fn from_lua_multi(args: LuaMultiValue, lua: &Lua) -> LuaResult<Self> {
        let brewfile = ExpandedPath::from_lua_multi(args, lua)?;
        Ok(Self { brewfile })
    }
}

impl std::fmt::Display for BrewBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "brew-bundle {}", self.brewfile)
    }
}

impl AssertionType for BrewBundle {
    fn status(&self) -> Result<Status> {
        // Assert platform is Darwin (macOS)
        #[cfg(not(target_os = "macos"))]
        return Err(eyre!("brew-bundle only supported on Darwin/macOS"));

        // Assert brew binary exists
        Utils::assert_bin("brew")?;

        // First check: brew bundle check --no-upgrade
        let (_output, exit_code) = Utils::sh_with_envs(
            "brew",
            &[
                "bundle".to_string(),
                "check".to_string(),
                "--no-upgrade".to_string(),
                format!("--file={}", self.brewfile),
            ],
            &[("HOMEBREW_NO_AUTO_UPDATE", "true")],
        )?;

        if exit_code != 0 {
            return Ok(Status::Missing);
        }

        // Second check: brew bundle check (without --no-upgrade)
        let (_output, exit_code) = Utils::sh_with_envs(
            "brew",
            &[
                "bundle".to_string(),
                "check".to_string(),
                format!("--file={}", self.brewfile),
            ],
            &[("HOMEBREW_NO_AUTO_UPDATE", "true")],
        )?;

        if exit_code != 0 {
            return Ok(Status::ConflictUpgrade(Conflict {
                expected: "up-to-date packages".to_string(),
                actual: "packages need upgrade".to_string(),
            }));
        }

        Ok(Status::Ok)
    }

    fn install(&self) -> Result<()> {
        // Assert platform is Darwin (macOS)
        #[cfg(not(target_os = "macos"))]
        return Err(eyre!("brew-bundle only supported on Darwin/macOS"));

        // Assert brew binary exists
        Utils::assert_bin("brew")?;

        let (_output, exit_code) = Utils::sh(
            "brew",
            &[
                "bundle".to_string(),
                "install".to_string(),
                format!("--file={}", self.brewfile),
            ],
        )?;

        if exit_code != 0 {
            return Err(eyre!("Failed to install brew bundle"));
        }

        debug!("installed: {}", self);
        Ok(())
    }

    fn upgrade(&self) -> Result<()> {
        todo!()
    }

    fn remove(&self) -> Result<()> {
        todo!()
    }
}
