use mlua::prelude::*;
use regex::Regex;
use std::env;
use std::ffi::OsStr;
use std::ops::Deref;
use std::path::Path;
use std::process::Command;
use std::str::FromStr;
use std::sync::LazyLock;
use tracing::debug;

use crate::error::FrorkError;
use crate::error::SharedError;

/// What one run of a command produced: each stream captured separately,
/// plus its exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub status: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpandedPath(String);

impl ExpandedPath {
    pub fn new(path: &str) -> Result<Self, FrorkError> {
        Ok(Self(Utils::expand_path(path)?))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<ExpandedPath> for String {
    fn from(path: ExpandedPath) -> Self {
        path.0
    }
}

impl FromStr for ExpandedPath {
    type Err = FrorkError;

    fn from_str(path: &str) -> Result<Self, FrorkError> {
        Self::new(path)
    }
}

impl FromLua for ExpandedPath {
    fn from_lua(value: LuaValue, lua: &Lua) -> LuaResult<Self> {
        let path_str = String::from_lua(value, lua)?;
        Self::new(&path_str).map_err(FrorkError::into_lua_error)
    }
}

impl Deref for ExpandedPath {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for ExpandedPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl AsRef<OsStr> for ExpandedPath {
    fn as_ref(&self) -> &OsStr {
        OsStr::new(&self.0)
    }
}

impl AsRef<Path> for ExpandedPath {
    fn as_ref(&self) -> &Path {
        Path::new(&self.0)
    }
}

impl std::fmt::Display for ExpandedPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub struct Utils;

impl Utils {
    pub fn chomp(s: &str) -> String {
        s.trim_end_matches(['\n', '\r']).to_string()
    }

    pub fn dirname(path: &str) -> Result<Option<String>, FrorkError> {
        let expanded_path = Self::expand_path(path)?;
        let parent = Path::new(&expanded_path).parent();
        Ok(parent.and_then(|p| p.to_str().map(|s| s.to_string())))
    }

    pub fn expand_path(path: &str) -> Result<String, FrorkError> {
        Self::expand_path_with(path, |name| env::var(name).ok())
    }

    /// Expands `~` and `$VAR` references against the given lookup rather
    /// than the process environment.
    pub fn expand_path_with(
        path: &str,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<String, FrorkError> {
        // The pattern is a literal, so it either always compiles or never does.
        #[allow(clippy::unwrap_used)]
        static ENV_VAR_REGEX: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"\$([A-Za-z_][A-Za-z0-9_]*)").unwrap());

        let mut expanded = path.to_string();

        if let Some(rest) = expanded.strip_prefix('~') {
            if rest.is_empty() || rest.starts_with('/') {
                let home = lookup("HOME").ok_or(FrorkError::MissingHome)?;
                expanded = format!("{home}{rest}");
            } else {
                return Err(FrorkError::UnsupportedTilde {
                    path: path.to_string(),
                });
            }
        }

        let mut missing = Vec::new();
        // Groups 0 and 1 are non-optional in the pattern, so a match
        // guarantees both are present.
        #[allow(clippy::unwrap_used)]
        let replaced = ENV_VAR_REGEX.replace_all(&expanded, |caps: &regex::Captures| {
            let var_name = caps.get(1).unwrap().as_str();
            match lookup(var_name) {
                Some(value) => value,
                None => {
                    missing.push(var_name.to_string());
                    caps.get(0).unwrap().as_str().to_string()
                }
            }
        });
        expanded = replaced.to_string();

        if !missing.is_empty() {
            return Err(FrorkError::UnsetVariables { names: missing });
        }

        Ok(expanded)
    }

    pub fn platform() -> Result<String, FrorkError> {
        let output = Self::sh("uname", &["-s"])?;
        Ok(Self::chomp(&output.stdout).to_lowercase())
    }

    pub fn sh<T: AsRef<str>>(cmd: &str, args: &[T]) -> Result<CommandOutput, FrorkError> {
        Self::sh_with_envs(cmd, args, &[])
    }

    pub fn sh_with_envs<T: AsRef<str>>(
        cmd: &str,
        args: &[T],
        env_vars: &[(&str, &str)],
    ) -> Result<CommandOutput, FrorkError> {
        let args_display: Vec<&str> = args.iter().map(|arg| arg.as_ref()).collect();
        debug!(
            "Executing command: {} with args: {} and envs: {:?}",
            cmd,
            args_display.join(" "),
            env_vars
        );

        let mut command = Command::new(cmd);
        command.args(args.iter().map(|arg| arg.as_ref()));
        command.envs(env_vars.iter().map(|(k, v)| (*k, *v)));

        let output = command
            .output()
            .map_err(|source| FrorkError::CommandSpawn {
                command: cmd.to_string(),
                source: SharedError::new(source),
            })?;

        // Both streams become Fennel strings and error-message text, which
        // non-UTF-8 output is unusable as either way.
        #[allow(clippy::disallowed_methods)]
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        #[allow(clippy::disallowed_methods)]
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let code = output.status.code();
        let status = code.ok_or_else(|| FrorkError::CommandSignaled {
            command: cmd.to_string(),
        })?;
        let stdout_trimmed = stdout.trim();
        let stderr_trimmed = stderr.trim();
        debug!(
            "Command '{}' completed with status {}, stdout: {}, stderr: {}",
            // The subscriber test exercises these, but nightly maps the
            // lazy format regions to a counter that never fires.
            cmd,            // cov-excl-line
            status,         // cov-excl-line
            stdout_trimmed, // cov-excl-line
            stderr_trimmed  // cov-excl-line
        );
        Ok(CommandOutput {
            stdout,
            stderr,
            status,
        })
    }

    pub fn assert_bin(bin_name: &str) -> Result<(), FrorkError> {
        let output = Self::sh("which", &[bin_name])?;

        if output.status == 0 {
            debug!("Binary '{}' found in PATH", bin_name);
            Ok(())
        } else {
            Err(FrorkError::MissingBinary {
                bin: bin_name.to_string(),
            })
        }
    }
}

impl IntoLua for Utils {
    fn into_lua(self, lua: &Lua) -> LuaResult<LuaValue> {
        let utils_table = lua.create_table()?;

        // Each function is bound before being set: a multi-line `set(...)?`
        // strands the `?` failure path on its own line, where it reads as an
        // uncovered line that no test can reach.
        let expand_path = lua.create_function(|_lua, path: String| {
            Utils::expand_path(&path).map_err(FrorkError::into_lua_error)
        })?;
        utils_table.set("expand-path", expand_path)?;

        let dirname = lua.create_function(|_lua, path: String| {
            Utils::dirname(&path).map_err(FrorkError::into_lua_error)
        })?;
        utils_table.set("dirname", dirname)?;

        let chomp = lua.create_function(|_lua, s: String| Ok(Utils::chomp(&s)))?;
        utils_table.set("chomp", chomp)?;

        let platform = Utils::platform().map_err(FrorkError::into_lua_error)?;
        utils_table.set("platform", platform)?;

        let sh = lua.create_function(|_lua, args: LuaVariadic<String>| {
            let mut args_iter = args.into_iter();
            let missing = || FrorkError::MissingCommand { function: "sh" }.into_lua_error();
            let cmd = args_iter.next().ok_or_else(missing)?;
            let cmd_args: Vec<String> = args_iter.collect();

            // A command that can't run reports as nils and -1, so scripts
            // can probe with it; the reason only goes to the log.
            Ok(Utils::sh(&cmd, &cmd_args)
                .map(|out| (Some(out.stdout), Some(out.stderr), out.status))
                .unwrap_or_else(|err| {
                    let reason = err.full_message();
                    debug!("sh: {reason}"); // cov-excl-line
                    (None, None, -1)
                }))
        })?;
        utils_table.set("sh", sh)?;

        let sh_strict = lua.create_function(|_lua, args: LuaVariadic<String>| {
            let mut args_iter = args.into_iter();
            let missing = || FrorkError::MissingCommand { function: "sh!" }.into_lua_error();
            let cmd = args_iter.next().ok_or_else(missing)?;
            let cmd_args: Vec<String> = args_iter.collect();

            Utils::sh(&cmd, &cmd_args)
                .map(|out| (Some(out.stdout), Some(out.stderr), out.status))
                .map_err(FrorkError::into_lua_error)
        })?;
        utils_table.set("sh!", sh_strict)?;

        let assert_bin = lua.create_function(|_lua, bin_name: String| {
            Utils::assert_bin(&bin_name).map_err(FrorkError::into_lua_error)
        })?;
        utils_table.set("assert-bin", assert_bin)?;

        Ok(LuaValue::Table(utils_table))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Cause;
    use std::env;

    /// A lookup backed by fixed pairs, standing in for the process
    /// environment so tests never mutate it.
    fn lookup_from<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn test_expand_path_with_multiple_env_vars() {
        let vars = [("TEST_VAR1", "value1"), ("TEST_VAR2", "value2")];
        let result =
            Utils::expand_path_with("/$TEST_VAR1/path/$TEST_VAR2/file", lookup_from(&vars));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "/value1/path/value2/file");
    }

    #[test]
    fn test_expand_path_mixed_existing_and_missing() {
        let vars = [("EXISTING_VAR", "exists")];
        let result =
            Utils::expand_path_with("/$EXISTING_VAR/path/$MISSING_VAR/file", lookup_from(&vars));
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Environment variables not found: MISSING_VAR")
        );
    }

    #[test]
    fn test_expand_path_tilde_and_env_var() {
        let vars = [("HOME", "/home/user"), ("TEST_VAR", "test")];
        let result = Utils::expand_path_with("~/$TEST_VAR/file", lookup_from(&vars));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "/home/user/test/file");
    }

    #[test]
    fn test_expand_path_bare_tilde() {
        let vars = [("HOME", "/home/user")];
        let result = Utils::expand_path_with("~", lookup_from(&vars));
        assert_eq!(result.unwrap(), "/home/user");
    }

    #[test]
    fn test_expand_path_rejects_a_user_tilde() {
        let vars = [("HOME", "/home/user")];
        let error = Utils::expand_path_with("~root/x", lookup_from(&vars)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("only '~' and '~/' are supported")
        );
    }

    #[test]
    fn test_expand_path_without_home() {
        let error = Utils::expand_path_with("~/x", |_| None).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("HOME environment variable not set")
        );
    }

    #[test]
    fn test_sh_through_lua() {
        let lua = mlua::Lua::new();
        let utils = Utils {};
        let utils_table = utils.into_lua(&lua).unwrap();

        lua.globals().set("utils", utils_table).unwrap();

        let result: (Option<String>, Option<String>, i32) = lua
            .load(r#"return utils.sh("echo", "hello", "world")"#)
            .eval()
            .unwrap();

        assert_eq!(result.0.unwrap().trim(), "hello world");
        assert_eq!(result.1.unwrap(), "");
        assert_eq!(result.2, 0);

        // stderr arrives on its own, not folded into stdout.
        let result: (Option<String>, Option<String>, i32) = lua
            .load(r#"return utils.sh("sh", "-c", "echo oops >&2")"#)
            .eval()
            .unwrap();

        assert_eq!(result.0.unwrap(), "");
        assert_eq!(result.1.unwrap().trim(), "oops");
        assert_eq!(result.2, 0);

        let result: (Option<String>, Option<String>, i32) =
            lua.load(r#"return utils.sh("false")"#).eval().unwrap();

        assert_eq!(result.2, 1);

        let result: (Option<String>, Option<String>, i32) = lua
            .load(r#"return utils["sh!"]("sh", "-c", "exit 42")"#)
            .eval()
            .unwrap();

        assert_eq!(result.2, 42);
    }

    #[test]
    fn test_sh_through_lua_invalid_arguments() {
        let lua = mlua::Lua::new();
        let utils = Utils {};
        let utils_table = utils.into_lua(&lua).unwrap();

        lua.globals().set("utils", utils_table).unwrap();

        let result = lua
            .load(r#"return utils.sh()"#)
            .eval::<(Option<String>, Option<String>, i32)>();
        assert!(result.is_err());

        let error = result.unwrap_err();
        assert!(error.to_string().contains("sh requires at least a command"));
        let error = Cause::from(error);
        assert!(matches!(
            error.frork(),
            Some(FrorkError::MissingCommand { function: "sh" })
        ));

        // `sh!` shares the argument check but propagates instead of returning nil.
        let result = lua
            .load(r#"return utils["sh!"]()"#)
            .eval::<(Option<String>, Option<String>, i32)>();
        let error = Cause::from(result.unwrap_err());
        assert!(matches!(
            error.frork(),
            Some(FrorkError::MissingCommand { function: "sh!" })
        ));
    }

    #[test]
    fn test_expanded_path_conversions() {
        // Reading the environment is thread-safe; only mutation is not.
        let expected = format!("/tmp/{}", env::var("HOME").unwrap());

        let path = ExpandedPath::new("/tmp/$HOME").unwrap();

        assert_eq!(path.as_str(), expected);
        assert_eq!(&*path, expected);
        assert_eq!(AsRef::<str>::as_ref(&path), expected);
        assert_eq!(AsRef::<OsStr>::as_ref(&path), OsStr::new(&expected));
        assert_eq!(AsRef::<Path>::as_ref(&path), Path::new(&expected));
        assert_eq!(path.to_string(), expected);

        assert_eq!("/tmp/$HOME".parse::<ExpandedPath>().unwrap(), path);

        assert_eq!(String::from(path.clone()), expected);
        assert_eq!(path.into_string(), expected);
    }

    #[test]
    fn test_dirname() {
        assert_eq!(
            Utils::dirname("/tmp/a/b.txt").unwrap().as_deref(),
            Some("/tmp/a")
        );
        // The filesystem root has no parent.
        assert_eq!(Utils::dirname("/").unwrap(), None);
    }

    #[test]
    fn test_assert_bin() {
        assert!(Utils::assert_bin("sh").is_ok());

        let error = Utils::assert_bin("frork-does-not-exist").unwrap_err();
        assert!(error.to_string().contains("not found in PATH"));
    }

    #[test]
    fn test_sh_reports_a_spawn_failure_with_its_source() {
        let error = Utils::sh("frork-does-not-exist", &[] as &[&str]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Failed to execute command 'frork-does-not-exist'"
        );
        // The io failure rides behind SharedError; a bare os error ends
        // the chain there.
        use std::error::Error as _;
        let source = error.source().expect("the io failure is the source");
        assert!(source.source().is_none());
    }

    #[test]
    fn test_sh_errors_when_a_command_is_killed_by_a_signal() {
        // A signal leaves no exit code, so the run cannot become a
        // CommandOutput.
        let error = Utils::sh("sh", &["-c", "kill -9 $$"]).unwrap_err();
        assert_eq!(error.to_string(), "Command 'sh' terminated by signal");
    }

    /// `uname -s`, lowercased — what `Utils::platform` is expected to report.
    #[cfg(target_os = "macos")]
    fn expected_platform() -> &'static str {
        "darwin"
    }

    #[cfg(not(target_os = "macos"))]
    fn expected_platform() -> &'static str {
        "linux"
    }

    #[test]
    fn test_platform() {
        assert_eq!(Utils::platform().unwrap(), expected_platform());
    }

    #[test]
    fn test_sh_reports_minus_one_when_the_command_cannot_spawn() {
        let lua = mlua::Lua::new();
        let utils_table = Utils {}.into_lua(&lua).unwrap();
        lua.globals().set("utils", utils_table).unwrap();

        // A missing binary fails to spawn, which is distinct from running and
        // exiting non-zero: `sh` reports nil and the -1 sentinel.
        let (stdout, stderr, exit_code): (Option<String>, Option<String>, i32) = lua
            .load(r#"return utils.sh("frork-does-not-exist")"#)
            .eval()
            .unwrap();

        assert_eq!(stdout, None);
        assert_eq!(stderr, None);
        assert_eq!(exit_code, -1);
    }

    #[test]
    fn test_utils_lua_bindings() {
        let lua = mlua::Lua::new();
        let utils_table = Utils {}.into_lua(&lua).unwrap();
        lua.globals().set("utils", utils_table).unwrap();

        let platform: String = lua.load(r#"return utils.platform"#).eval().unwrap();
        assert_eq!(platform, expected_platform());

        lua.load(r#"utils["assert-bin"]("sh")"#).exec().unwrap();

        let missing = lua
            .load(r#"utils["assert-bin"]("frork-does-not-exist")"#)
            .exec();
        assert!(missing.is_err());
    }

    #[test]
    fn test_sh_debug_logging() {
        // The debug! arguments are evaluated lazily, so they only run with a
        // subscriber active at DEBUG level.
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer()
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            let output =
                Utils::sh_with_envs("sh", &["-c", "echo $LOGGED"], &[("LOGGED", "value")]).unwrap();
            assert_eq!(output.stdout.trim(), "value");
            assert_eq!(output.status, 0);
        });
    }
}
