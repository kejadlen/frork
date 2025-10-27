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

    pub fn dirname(path: &str) -> Result<Option<String>> {
        let expanded_path = Self::expand_path(path)?;
        let parent = Path::new(&expanded_path).parent();
        Ok(parent.and_then(|p| p.to_str().map(|s| s.to_string())))
    }

    pub fn expand_path(path: &str) -> Result<String> {
        static ENV_VAR_REGEX: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"\$([A-Za-z_][A-Za-z0-9_]*)").unwrap());

        let mut expanded = path.to_string();

        // Expand tilde
        if expanded.starts_with('~') {
            let home = env::var("HOME").map_err(|_| eyre!("HOME environment variable not set"))?;
            expanded = expanded.replacen('~', &home, 1);
        }

        // Expand environment variables
        let mut missing = Vec::new();
        expanded = ENV_VAR_REGEX
            .replace_all(&expanded, |caps: &regex::Captures| {
                let var_name = caps.get(1).unwrap().as_str();
                match env::var(var_name) {
                    Ok(value) => value,
                    Err(_) => {
                        missing.push(var_name.to_string());
                        caps.get(0).unwrap().as_str().to_string()
                    }
                }
            })
            .to_string();

        if !missing.is_empty() {
            return Err(eyre!(
                "Environment variables not found: {}",
                missing.join(", ")
            ));
        }

        Ok(expanded)
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
            lua.create_function(|_lua, path: String| {
                Utils::expand_path(&path).map_err(LuaError::external)
            })?,
        )?;
        utils_table.set(
            "dirname",
            lua.create_function(|_lua, path: String| {
                Utils::dirname(&path).map_err(LuaError::external)
            })?,
        )?;
        utils_table.set(
            "chomp",
            lua.create_function(|_lua, s: String| Ok(Utils::chomp(&s)))?,
        )?;
        utils_table.set(
            "sh",
            lua.create_function(|_lua, args: LuaVariadic<String>| {
                let mut args_iter = args.into_iter();
                let cmd = args_iter.next().ok_or_else(|| {
                    LuaError::external(FrorkError::InvalidArguments(
                        "sh requires at least a command".to_string(),
                    ))
                })?;
                let cmd_args: Vec<String> = args_iter.collect();

                let result = Utils::sh(&cmd, &cmd_args)
                    .map(|(stdout, status)| (Some(stdout), status))
                    .unwrap_or((None, -1));
                Ok(result)
            })?,
        )?;

        Ok(LuaValue::Table(utils_table))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_expand_path_no_variables() {
        let result = Utils::expand_path("/path/to/file");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "/path/to/file");
    }

    #[test]
    fn test_expand_path_with_tilde() {
        let home = env::var("HOME").unwrap();
        let result = Utils::expand_path("~/file");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), format!("{}/file", home));
    }

    #[test]
    fn test_expand_path_with_multiple_env_vars() {
        unsafe {
            env::set_var("TEST_VAR1", "value1");
            env::set_var("TEST_VAR2", "value2");
        }
        let result = Utils::expand_path("/$TEST_VAR1/path/$TEST_VAR2/file");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "/value1/path/value2/file");
        unsafe {
            env::remove_var("TEST_VAR1");
            env::remove_var("TEST_VAR2");
        }
    }

    #[test]
    fn test_expand_path_with_multiple_missing_env_vars() {
        let result = Utils::expand_path("/$MISSING1/path/$MISSING2/file");
        assert!(result.is_err());
        let error = result.unwrap_err();
        let error_str = error.to_string();
        assert!(error_str.contains("Environment variables not found:"));
        assert!(error_str.contains("MISSING1"));
        assert!(error_str.contains("MISSING2"));
    }

    #[test]
    fn test_expand_path_mixed_existing_and_missing() {
        unsafe {
            env::set_var("EXISTING_VAR", "exists");
        }
        let result = Utils::expand_path("/$EXISTING_VAR/path/$MISSING_VAR/file");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Environment variables not found: MISSING_VAR")
        );
        unsafe {
            env::remove_var("EXISTING_VAR");
        }
    }

    #[test]
    fn test_expand_path_tilde_and_env_var() {
        let home = env::var("HOME").unwrap();
        unsafe {
            env::set_var("TEST_VAR", "test");
        }
        let result = Utils::expand_path("~/$TEST_VAR/file");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), format!("{}/test/file", home));
        unsafe {
            env::remove_var("TEST_VAR");
        }
    }

    #[test]
    fn test_sh_through_lua() {
        let lua = mlua::Lua::new();
        let utils = Utils {};
        let utils_table = utils.into_lua(&lua).unwrap();

        lua.globals().set("utils", utils_table).unwrap();

        // Test successful command
        let result: (Option<String>, i32) = lua
            .load(r#"return utils.sh("echo", "hello", "world")"#)
            .eval()
            .unwrap();

        assert_eq!(result.0.unwrap().trim(), "hello world");
        assert_eq!(result.1, 0);

        // Test command with non-zero exit
        let result: (Option<String>, i32) = lua.load(r#"return utils.sh("false")"#).eval().unwrap();

        assert_eq!(result.1, 1);
    }

    #[test]
    fn test_sh_through_lua_invalid_arguments() {
        let lua = mlua::Lua::new();
        let utils = Utils {};
        let utils_table = utils.into_lua(&lua).unwrap();

        lua.globals().set("utils", utils_table).unwrap();

        // Test with no arguments - should fail
        let result = lua
            .load(r#"return utils.sh()"#)
            .eval::<(Option<String>, i32)>();
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert!(error.to_string().contains("sh requires at least a command"));
    }
}
