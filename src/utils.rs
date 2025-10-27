use color_eyre::{Result, eyre::eyre};
use mlua::prelude::*;
use regex::Regex;
use std::env;
use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;
use tracing::debug;

use crate::errors::FrorkError;

pub struct Utils;

impl Utils {
    pub fn chomp(s: &str) -> String {
        s.trim_end_matches('\n').trim_end_matches('\r').to_string()
    }

    pub fn dirname(path: &str) -> Option<String> {
        let expanded_path = Self::expand_path(path);
        let parent = Path::new(&expanded_path).parent()?;
        parent.to_str().map(|s| s.to_string())
    }

    pub fn expand_path(path: &str) -> String {
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

    pub fn sh(cmd: &str, args: &[String]) -> Result<(String, i32)> {
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
