use fs_err as fs;
use miette::IntoDiagnostic as _;
use miette::Result;
use miette::miette;
use mlua::prelude::*;
use serde::Deserialize;
use std::path::Path;
use std::path::PathBuf;
use tracing::debug;
use tracing::error;
use tracing::info;
use tracing::warn;

use crate::error::FrorkError;
use crate::utils::ExpandedPath;
use crate::utils::Utils;

pub trait AssertionType: std::fmt::Display {
    fn status(&self) -> Result<Status>;
    fn install(&self) -> Result<()>;
    fn upgrade(&self) -> Result<()>;

    // Not yet called — will be used when `frork remove` is implemented.
    // cov-excl-start
    #[allow(dead_code)]
    fn remove(&self) -> Result<()> {
        todo!()
    }
    // cov-excl-stop
}

#[derive(Debug)]
pub enum Status {
    Ok,
    Missing,
    ConflictUpgrade(Conflict),
}

impl FromLuaMulti for Status {
    fn from_lua_multi(values: LuaMultiValue, lua: &Lua) -> LuaResult<Self> {
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
            .map_err(|e| miette!("Failed to create assertion type: {e}"))
    }
}

// --- Built-in assertion types ---

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

impl Symlink {
    /// Where the link will point, resolving a relative source against the
    /// link's directory as the OS does, not the working directory.
    fn resolved_source(&self) -> PathBuf {
        let target = Path::new(&self.target);
        target.parent().unwrap_or(target).join(&self.source)
    }
}

impl std::fmt::Display for Symlink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "symlink {} {}", self.target, self.source)
    }
}

impl AssertionType for Symlink {
    fn status(&self) -> Result<Status> {
        // Unlike `Path::exists`, this doesn't follow the link, so a link to a
        // source that doesn't exist yet still counts as present.
        if fs::symlink_metadata(&self.target).is_err() {
            return Ok(Status::Missing);
        }

        if let Ok(link_target) = fs::read_link(&self.target) {
            if link_target == Path::new(&self.source) {
                Ok(Status::Ok)
            } else {
                Ok(Status::ConflictUpgrade(Conflict {
                    expected: self.source.to_string(),
                    actual: format!("points to {}", link_target.display()),
                }))
            }
        } else {
            Ok(Status::ConflictUpgrade(Conflict {
                expected: format!("symlink to {}", self.source),
                actual: "not a symlink".to_string(),
            }))
        }
    }

    fn install(&self) -> Result<()> {
        use std::os::unix::fs;
        // Scripts may create the source after the link, so a dangling link
        // is allowed, but it's more often a typo than intended.
        if !self.resolved_source().exists() {
            warn!("{self}: source does not exist, linking anyway");
        }
        fs::symlink(&self.source, &self.target)
            .map_err(|e| miette!("Failed to create symlink: {e}"))?;
        debug!("created: {}", self);
        Ok(())
    }

    // cov-excl-start
    fn upgrade(&self) -> Result<()> {
        todo!()
    }
    // cov-excl-stop
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
            return Ok(Status::Ok);
        }

        // Unlike `Path::exists`, this doesn't follow the link, so a dangling
        // link reads as something in the way rather than nothing there.
        let Ok(metadata) = fs::symlink_metadata(path) else {
            return Ok(Status::Missing);
        };
        Ok(Status::ConflictUpgrade(Conflict {
            expected: "directory".to_string(),
            actual: file_kind(metadata.file_type()).to_string(),
        }))
    }

    fn install(&self) -> Result<()> {
        fs::create_dir_all(&self.path).into_diagnostic()?;
        debug!("created: {}", self);
        Ok(())
    }

    // cov-excl-start
    fn upgrade(&self) -> Result<()> {
        todo!()
    }
    // cov-excl-stop
}

/// Names what occupies a path that should hold something else.
fn file_kind(file_type: std::fs::FileType) -> &'static str {
    use std::os::unix::fs::FileTypeExt as _;
    if file_type.is_symlink() {
        "symlink"
    } else if file_type.is_file() {
        "file"
    } else if file_type.is_fifo() {
        "fifo"
    } else if file_type.is_socket() {
        "socket"
    } else if file_type.is_block_device() || file_type.is_char_device() {
        "device"
    } else {
        // Only a directory is left, which callers have already ruled out.
        "directory" // cov-excl-line
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
        Utils::assert_bin("git")?;

        let dir_path = Path::new(&self.dir);

        if !dir_path.exists() {
            return Ok(Status::Missing);
        }

        if !dir_path.is_dir() {
            return Ok(Status::ConflictUpgrade(Conflict {
                expected: "directory".to_string(),
                actual: "file".to_string(),
            }));
        }

        // An empty directory can still be cloned into, so it reads as
        // not-yet-cloned.
        let mut entries = fs::read_dir(dir_path).into_diagnostic()?;
        if entries.next().is_none() {
            return Ok(Status::Missing);
        }

        let config_args = [
            "-C",
            self.dir.as_str(),
            "config",
            "--get",
            "remote.origin.url",
        ];
        let (remote_output, exit_code) = Utils::sh("git", &config_args)?;

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
        Utils::assert_bin("git")?;

        let (_output, exit_code) = Utils::sh("git", &["clone", &self.remote_url, &self.dir])?;

        if exit_code != 0 {
            return Err(miette!("Failed to clone git repository"));
        }

        debug!("created: {}", self);
        Ok(())
    }

    // cov-excl-start
    fn upgrade(&self) -> Result<()> {
        todo!()
    }
    // cov-excl-stop
}

/// A seam over shelling out, so assertion types can be exercised without
/// running the real command. Production code always uses [`SystemRunner`];
/// only tests substitute anything else.
pub trait CommandRunner {
    fn has_bin(&self, bin: &str) -> bool;
    fn run(&self, cmd: &str, args: &[&str]) -> Result<(String, i32)>;
    fn run_with_envs(
        &self,
        cmd: &str,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> Result<(String, i32)>;

    fn require_bin(&self, bin: &str) -> Result<()> {
        if self.has_bin(bin) {
            Ok(())
        } else {
            Err(miette!("Required binary '{bin}' not found in PATH"))
        }
    }
}

pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn has_bin(&self, bin: &str) -> bool {
        Utils::assert_bin(bin).is_ok()
    }

    fn run(&self, cmd: &str, args: &[&str]) -> Result<(String, i32)> {
        Utils::sh(cmd, args)
    }

    fn run_with_envs(
        &self,
        cmd: &str,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> Result<(String, i32)> {
        Utils::sh_with_envs(cmd, args, envs)
    }
}

pub struct Brew<R = SystemRunner> {
    runner: R,
}

// Only the production configuration is constructible from Lua; tests build
// Brew directly with a fake runner.
impl FromLuaMulti for Brew<SystemRunner> {
    fn from_lua_multi(_args: LuaMultiValue, _lua: &Lua) -> LuaResult<Self> {
        Ok(Self {
            runner: SystemRunner,
        })
    }
}

impl<R> std::fmt::Display for Brew<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "brew")
    }
}

impl<R: CommandRunner> AssertionType for Brew<R> {
    fn status(&self) -> Result<Status> {
        if !self.runner.has_bin("brew") {
            return Ok(Status::Missing);
        }

        let (_output, exit_code) = self.runner.run("brew", &["--version"])?;
        if exit_code == 0 {
            Ok(Status::Ok)
        } else {
            Ok(Status::Missing)
        }
    }

    // This needs sudo — figure out how to make this work through frork.
    fn install(&self) -> Result<()> {
        let install_args = [
            "-c",
            r#"/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)""#,
        ];
        let (output, exit_code) = self.runner.run("bash", &install_args)?;

        if exit_code != 0 {
            return Err(miette!("Failed to install Homebrew: {}", output));
        }

        debug!("installed: {}", self);
        Ok(())
    }

    // cov-excl-start
    fn upgrade(&self) -> Result<()> {
        todo!()
    }
    // cov-excl-stop
}

pub struct BrewBundle<R = SystemRunner> {
    pub brewfile: ExpandedPath,
    // Only read on macOS; every other platform bails before shelling out.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    runner: R,
}

impl FromLuaMulti for BrewBundle<SystemRunner> {
    fn from_lua_multi(args: LuaMultiValue, lua: &Lua) -> LuaResult<Self> {
        let brewfile = ExpandedPath::from_lua_multi(args, lua)?;
        Ok(Self {
            brewfile,
            runner: SystemRunner,
        })
    }
}

impl<R> std::fmt::Display for BrewBundle<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "brew-bundle {}", self.brewfile)
    }
}

impl<R: CommandRunner> AssertionType for BrewBundle<R> {
    #[cfg(not(target_os = "macos"))]
    fn status(&self) -> Result<Status> {
        Err(miette!("brew-bundle only supported on Darwin/macOS"))
    }

    #[cfg(target_os = "macos")]
    fn status(&self) -> Result<Status> {
        self.runner.require_bin("brew")?;

        let file_arg = format!("--file={}", self.brewfile);
        let no_auto_update = [("HOMEBREW_NO_AUTO_UPDATE", "true")];

        let strict_args = ["bundle", "check", "--no-upgrade", file_arg.as_str()];
        let (_output, exit_code) =
            self.runner
                .run_with_envs("brew", &strict_args, &no_auto_update)?;

        if exit_code != 0 {
            return Ok(Status::Missing);
        }

        let check_args = ["bundle", "check", file_arg.as_str()];
        let (_output, exit_code) =
            self.runner
                .run_with_envs("brew", &check_args, &no_auto_update)?;

        if exit_code != 0 {
            return Ok(Status::ConflictUpgrade(Conflict {
                expected: "up-to-date packages".to_string(),
                actual: "packages need upgrade".to_string(),
            }));
        }

        Ok(Status::Ok)
    }

    #[cfg(not(target_os = "macos"))]
    fn install(&self) -> Result<()> {
        Err(miette!("brew-bundle only supported on Darwin/macOS"))
    }

    #[cfg(target_os = "macos")]
    fn install(&self) -> Result<()> {
        self.runner.require_bin("brew")?;

        let file_arg = format!("--file={}", self.brewfile);
        let (_output, exit_code) = self
            .runner
            .run("brew", &["bundle", "install", file_arg.as_str()])?;

        if exit_code != 0 {
            return Err(miette!("Failed to install brew bundle"));
        }

        debug!("installed: {}", self);
        Ok(())
    }

    // cov-excl-start
    fn upgrade(&self) -> Result<()> {
        todo!()
    }
    // cov-excl-stop
}

pub struct Debug {
    pub status_fn: Option<LuaFunction>,
    pub install_fn: Option<LuaFunction>,
}

impl FromLuaMulti for Debug {
    fn from_lua_multi(args: LuaMultiValue, lua: &Lua) -> LuaResult<Self> {
        let table = LuaTable::from_lua_multi(args, lua)?;
        let status_fn: Option<LuaFunction> = table.get("status")?;
        let install_fn: Option<LuaFunction> = table.get("install")?;
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
                .map_err(|e| miette!("Debug status function failed: {e}"))?;
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
            .ok_or_else(|| miette!("Install not implemented for debug assertion"))?;
        install_fn
            .call::<()>(LuaMultiValue::new())
            .map_err(|e| miette!("Debug install function failed: {e}"))?;
        Ok(())
    }

    // cov-excl-start
    fn upgrade(&self) -> Result<()> {
        todo!()
    }
    // cov-excl-stop
}

// --- Lua custom assertion types ---

#[derive(Clone)]
pub struct LuaAssertionType {
    pub display_fn: Option<LuaFunction>,
    pub status_fn: LuaFunction,
    pub install_fn: LuaFunction,
    pub upgrade_fn: Option<LuaFunction>,
}

impl FromLua for LuaAssertionType {
    fn from_lua(value: LuaValue, lua: &Lua) -> LuaResult<Self> {
        let table = LuaTable::from_lua(value, lua)?;

        let display_fn: Option<LuaFunction> = table.get("display")?;
        let status_fn: LuaFunction = table.get("status")?;
        let install_fn: LuaFunction = table.get("install")?;
        let upgrade_fn: Option<LuaFunction> = table.get("upgrade")?;

        Ok(Self {
            display_fn,
            status_fn,
            install_fn,
            upgrade_fn,
        })
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
            let args = self
                .args
                .iter()
                .map(|v| v.to_string().unwrap_or_else(|_| "?".to_string()));
            std::iter::once(self.name.clone())
                .chain(args)
                .collect::<Vec<_>>()
                .join(" ")
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
        // A missing upgrade function falls back to install — re-running the
        // install is the default way to bring a conflicted assertion in line.
        if let Some(upgrade_fn) = self.assertion_type.upgrade_fn.as_ref() {
            upgrade_fn
                .call::<()>(self.args.clone())
                .map_err(FrorkError::from)?;
        } else {
            self.install()?;
        }
        debug!("upgraded: {}", self);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use tempfile::TempDir;

    fn multi(lua: &Lua, script: &str) -> LuaMultiValue {
        lua.load(script).eval::<LuaMultiValue>().unwrap()
    }

    fn expanded(path: &Path) -> ExpandedPath {
        path.to_str().unwrap().parse().unwrap()
    }

    // --- Status / Conflict ---

    fn status_from(lua: &Lua, script: &str) -> LuaResult<Status> {
        Status::from_lua_multi(multi(lua, script), lua)
    }

    #[test]
    fn test_status_from_lua_single_strings() {
        let lua = Lua::new();

        assert!(matches!(
            status_from(&lua, r#"return "ok""#),
            Ok(Status::Ok)
        ));
        assert!(matches!(
            status_from(&lua, r#"return "missing""#),
            Ok(Status::Missing)
        ));

        let err = status_from(&lua, r#"return "bogus""#).unwrap_err();
        assert!(err.to_string().contains("Invalid status string: 'bogus'"));
    }

    #[test]
    fn test_status_from_lua_rejects_non_string() {
        let lua = Lua::new();

        let err = status_from(&lua, r#"return true"#).unwrap_err();
        assert!(
            err.to_string()
                .contains("Expected single string or conflict-upgrade with table")
        );
    }

    // --- TypedFactory ---

    #[test]
    fn test_typed_factory_creates_assertion() {
        let lua = Lua::new();
        let factory: TypedFactory<Directory> = TypedFactory::default();

        let assertion = factory
            .create(&lua, multi(&lua, r#"return "/tmp""#))
            .unwrap();
        assert_eq!(assertion.to_string(), "directory /tmp");
    }

    #[test]
    fn test_typed_factory_reports_conversion_failure() {
        let lua = Lua::new();
        let factory: TypedFactory<Directory> = TypedFactory::new();

        // Box<dyn AssertionType> is not Debug, so unwrap_err is unavailable.
        let Err(error) = factory.create(&lua, multi(&lua, r#"return true"#)) else {
            panic!("expected a conversion failure"); // cov-excl-line
        };
        assert!(
            error
                .to_string()
                .contains("Failed to create assertion type")
        );
    }

    // --- Symlink ---

    #[test]
    fn test_symlink_status_conflicts() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source");
        fs::write(&source, "").unwrap();

        // A link pointing somewhere else conflicts.
        let other = dir.path().join("other");
        fs::write(&other, "").unwrap();
        let target = dir.path().join("link");
        std::os::unix::fs::symlink(&other, &target).unwrap();
        let symlink = Symlink {
            target: expanded(&target),
            source: expanded(&source),
        };
        let Status::ConflictUpgrade(conflict) = symlink.status().unwrap() else {
            panic!("expected a conflict for a wrong-target link"); // cov-excl-line
        };
        assert_eq!(conflict.expected, source.display().to_string());
        assert_eq!(conflict.actual, format!("points to {}", other.display()));

        // A non-symlink where the link goes conflicts too.
        let file = dir.path().join("file-target");
        fs::write(&file, "").unwrap();
        let symlink = Symlink {
            target: expanded(&file),
            source: expanded(&source),
        };
        let Status::ConflictUpgrade(conflict) = symlink.status().unwrap() else {
            panic!("expected a conflict for a non-symlink target"); // cov-excl-line
        };
        assert_eq!(
            conflict.expected,
            format!("symlink to {}", source.display())
        );
        assert_eq!(conflict.actual, "not a symlink");
    }

    #[test]
    fn test_symlink_install_failure() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source.txt");
        fs::write(&source, "").unwrap();
        let symlink = Symlink {
            target: expanded(&dir.path().join("missing-parent/link")),
            source: expanded(&source),
        };

        let error = symlink.install().unwrap_err();
        assert!(error.to_string().contains("Failed to create symlink"));
    }

    /// Runs `f` under a subscriber that records WARN and above, and returns
    /// what it logged.
    fn warnings_from(f: impl FnOnce()) -> String {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("log");
        let writer = std::sync::Arc::new(fs::File::create(&log).unwrap());
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .with_writer(writer)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        fs::read_to_string(&log).unwrap()
    }

    #[test]
    fn test_symlink_install_warns_about_a_missing_source() {
        let dir = TempDir::new().unwrap();
        let symlink = Symlink {
            target: expanded(&dir.path().join("link")),
            source: expanded(&dir.path().join("never-created")),
        };

        let warnings = warnings_from(|| symlink.install().unwrap());

        assert!(warnings.contains("source does not exist"), "{warnings}");
        assert!(matches!(symlink.status().unwrap(), Status::Ok));
    }

    #[test]
    fn test_symlink_install_resolves_a_relative_source_beside_the_link() {
        let dir = TempDir::new().unwrap();
        let links = dir.path().join("links");
        fs::create_dir(&links).unwrap();
        fs::write(links.join("present"), "").unwrap();

        // Neither name exists relative to the working directory, so only
        // resolving against the link's directory tells them apart.
        let present = Symlink {
            target: expanded(&links.join("to-present")),
            source: "present".parse().unwrap(),
        };
        let absent = Symlink {
            target: expanded(&links.join("to-absent")),
            source: "absent".parse().unwrap(),
        };

        assert_eq!(warnings_from(|| present.install().unwrap()), "");
        assert!(warnings_from(|| absent.install().unwrap()).contains("source does not exist"));
    }

    #[test]
    fn test_symlink_from_lua() {
        let lua = Lua::new();
        let symlink =
            Symlink::from_lua_multi(multi(&lua, r#"return "/tmp/link", "/tmp/source""#), &lua)
                .unwrap();

        assert_eq!(symlink.target.as_str(), "/tmp/link");
        assert_eq!(symlink.source.as_str(), "/tmp/source");
    }

    // --- Directory ---

    fn directory_conflict(path: &Path) -> Conflict {
        let directory = Directory {
            path: expanded(path),
        };
        let Status::ConflictUpgrade(conflict) = directory.status().unwrap() else {
            panic!("expected a conflict for {path:?}"); // cov-excl-line
        };
        assert_eq!(conflict.expected, "directory");
        conflict
    }

    #[test]
    fn test_directory_status_names_what_is_in_the_way() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let file = dir.path().join("file");
        fs::write(&file, "content").unwrap();
        let to_file = dir.path().join("to-file");
        symlink(&file, &to_file).unwrap();
        let dangling = dir.path().join("dangling");
        symlink(dir.path().join("nowhere"), &dangling).unwrap();
        let fifo = dir.path().join("fifo");
        let mkfifo = std::process::Command::new("mkfifo").arg(&fifo).status();
        assert!(mkfifo.unwrap().success());
        let socket = dir.path().join("socket");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();

        assert_eq!(directory_conflict(&file).actual, "file");
        assert_eq!(directory_conflict(&to_file).actual, "symlink");
        assert_eq!(directory_conflict(&dangling).actual, "symlink");
        assert_eq!(directory_conflict(&fifo).actual, "fifo");
        assert_eq!(directory_conflict(&socket).actual, "socket");
        assert_eq!(directory_conflict(Path::new("/dev/null")).actual, "device");
    }

    #[test]
    fn test_directory_status_follows_a_link_to_a_directory() {
        let dir = TempDir::new().unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(dir.path(), &link).unwrap();

        let directory = Directory {
            path: expanded(&link),
        };
        assert!(matches!(directory.status().unwrap(), Status::Ok));
    }

    #[test]
    fn test_directory_install_failure() {
        let dir = TempDir::new().unwrap();
        let blocker = dir.path().join("blocker");
        fs::write(&blocker, "not a directory").unwrap();

        let directory = Directory {
            path: expanded(&blocker.join("child")),
        };
        assert!(directory.install().is_err());
    }

    #[test]
    fn test_directory_from_lua() {
        let lua = Lua::new();
        let directory = Directory::from_lua_multi(multi(&lua, r#"return "/tmp""#), &lua).unwrap();

        assert_eq!(directory.path.as_str(), "/tmp");
    }

    // --- Git ---

    fn bare_origin(dir: &Path) -> String {
        let origin = dir.join("origin.git");
        let (_out, code) = Utils::sh("git", &["init", "--bare", origin.to_str().unwrap()]).unwrap();
        assert_eq!(code, 0);
        origin.to_str().unwrap().to_string()
    }

    #[test]
    fn test_git_lifecycle() {
        let dir = TempDir::new().unwrap();
        let remote_url = bare_origin(dir.path());
        let clone = dir.path().join("clone");

        let git = Git {
            dir: expanded(&clone),
            remote_url: remote_url.clone(),
        };

        assert_eq!(
            git.to_string(),
            format!("git {} {}", clone.display(), remote_url)
        );
        assert!(matches!(git.status().unwrap(), Status::Missing));

        git.install().unwrap();
        assert!(matches!(git.status().unwrap(), Status::Ok));
    }

    #[test]
    fn test_git_status_conflicts() {
        let dir = TempDir::new().unwrap();
        let remote_url = bare_origin(dir.path());

        // A file where a directory is expected.
        let file = dir.path().join("a-file");
        fs::write(&file, "content").unwrap();
        let git = Git {
            dir: expanded(&file),
            remote_url: remote_url.clone(),
        };
        let Status::ConflictUpgrade(conflict) = git.status().unwrap() else {
            panic!("expected a conflict for a non-directory path"); // cov-excl-line
        };
        assert_eq!(conflict.expected, "directory");
        assert_eq!(conflict.actual, "file");

        // An empty directory counts as missing rather than conflicting.
        let empty = dir.path().join("empty");
        fs::create_dir_all(&empty).unwrap();
        let git = Git {
            dir: expanded(&empty),
            remote_url: remote_url.clone(),
        };
        assert!(matches!(git.status().unwrap(), Status::Missing));

        // A non-empty directory that is not a repo has no remote to read.
        let not_a_repo = dir.path().join("not-a-repo");
        fs::create_dir_all(&not_a_repo).unwrap();
        fs::write(not_a_repo.join("file.txt"), "content").unwrap();
        let git = Git {
            dir: expanded(&not_a_repo),
            remote_url: remote_url.clone(),
        };
        let Status::ConflictUpgrade(conflict) = git.status().unwrap() else {
            panic!("expected a conflict when the remote cannot be read"); // cov-excl-line
        };
        assert_eq!(conflict.actual, "failed to get remote url");
    }

    #[test]
    fn test_git_install_failure() {
        let dir = TempDir::new().unwrap();
        let git = Git {
            dir: expanded(&dir.path().join("clone")),
            remote_url: dir.path().join("does-not-exist.git").display().to_string(),
        };

        let error = git.install().unwrap_err();
        assert!(error.to_string().contains("Failed to clone git repository"));
    }

    #[test]
    fn test_git_from_lua() {
        let lua = Lua::new();
        let git = Git::from_lua_multi(
            multi(&lua, r#"return "/tmp/repo", "https://example.com/r.git""#),
            &lua,
        )
        .unwrap();

        assert_eq!(git.dir.as_str(), "/tmp/repo");
        assert_eq!(git.remote_url, "https://example.com/r.git");
    }

    // --- CommandRunner ---

    /// Records every command it is asked to run and replays canned exit codes.
    #[derive(Default)]
    struct FakeRunner {
        missing_bins: Vec<&'static str>,
        exit_codes: RefCell<VecDeque<i32>>,
        calls: RefCell<Vec<String>>,
    }

    impl FakeRunner {
        fn new(exit_codes: &[i32]) -> Self {
            Self {
                exit_codes: RefCell::new(exit_codes.iter().copied().collect()),
                ..Default::default()
            }
        }

        fn without_bin(bin: &'static str) -> Self {
            Self {
                missing_bins: vec![bin],
                ..Default::default()
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl CommandRunner for FakeRunner {
        fn has_bin(&self, bin: &str) -> bool {
            !self.missing_bins.contains(&bin)
        }

        fn run(&self, cmd: &str, args: &[&str]) -> Result<(String, i32)> {
            self.run_with_envs(cmd, args, &[])
        }

        fn run_with_envs(
            &self,
            cmd: &str,
            args: &[&str],
            _envs: &[(&str, &str)],
        ) -> Result<(String, i32)> {
            self.calls
                .borrow_mut()
                .push(format!("{cmd} {}", args.join(" ")));
            let exit_code = self.exit_codes.borrow_mut().pop_front().unwrap_or(0);
            Ok((format!("output of {cmd}"), exit_code))
        }
    }

    #[test]
    fn test_system_runner_shells_out_for_real() {
        let runner = SystemRunner;

        assert!(runner.has_bin("sh"));
        assert!(!runner.has_bin("frork-does-not-exist"));

        let (stdout, exit_code) = runner.run("echo", &["hello"]).unwrap();
        assert_eq!(stdout.trim(), "hello");
        assert_eq!(exit_code, 0);

        let (stdout, exit_code) = runner
            .run_with_envs("sh", &["-c", "echo $RUNNER_VAR"], &[("RUNNER_VAR", "set")])
            .unwrap();
        assert_eq!(stdout.trim(), "set");
        assert_eq!(exit_code, 0);
    }

    #[test]
    fn test_command_runner_require_bin() {
        let runner = FakeRunner::without_bin("brew");

        assert!(runner.require_bin("git").is_ok());
        let error = runner.require_bin("brew").unwrap_err();
        assert!(error.to_string().contains("not found in PATH"));
    }

    // --- Brew ---

    #[test]
    fn test_brew_display_and_from_lua() {
        let lua = Lua::new();
        let brew = Brew::from_lua_multi(LuaMultiValue::new(), &lua).unwrap();

        assert_eq!(brew.to_string(), "brew");
    }

    #[test]
    fn test_brew_status_without_brew_installed() {
        let brew = Brew {
            runner: FakeRunner::without_bin("brew"),
        };

        assert!(matches!(brew.status().unwrap(), Status::Missing));
        assert!(brew.runner.calls().is_empty());
    }

    #[test]
    fn test_brew_status_reflects_version_exit_code() {
        let brew = Brew {
            runner: FakeRunner::new(&[0]),
        };
        assert!(matches!(brew.status().unwrap(), Status::Ok));
        assert_eq!(brew.runner.calls(), ["brew --version"]);

        // Present on PATH but not runnable.
        let brew = Brew {
            runner: FakeRunner::new(&[1]),
        };
        assert!(matches!(brew.status().unwrap(), Status::Missing));
    }

    #[test]
    fn test_brew_install() {
        let brew = Brew {
            runner: FakeRunner::new(&[0]),
        };
        brew.install().unwrap();
        assert!(brew.runner.calls()[0].contains("install.sh"));

        let brew = Brew {
            runner: FakeRunner::new(&[1]),
        };
        let error = brew.install().unwrap_err();
        assert!(error.to_string().contains("Failed to install Homebrew"));
    }

    // --- BrewBundle ---

    fn brew_bundle(runner: FakeRunner) -> BrewBundle<FakeRunner> {
        BrewBundle {
            brewfile: "/tmp/Brewfile".parse().unwrap(),
            runner,
        }
    }

    #[test]
    fn test_brew_bundle_display_and_from_lua() {
        let lua = Lua::new();
        let bundle =
            BrewBundle::from_lua_multi(multi(&lua, r#"return "/tmp/Brewfile""#), &lua).unwrap();

        assert_eq!(bundle.to_string(), "brew-bundle /tmp/Brewfile");
        assert_eq!(bundle.brewfile.as_str(), "/tmp/Brewfile");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn test_brew_bundle_is_macos_only() {
        let bundle = brew_bundle(FakeRunner::default());

        assert!(bundle.status().is_err());
        assert!(bundle.install().is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_brew_bundle_status_requires_brew() {
        let bundle = brew_bundle(FakeRunner::without_bin("brew"));

        assert!(bundle.status().unwrap_err().to_string().contains("brew"));
        assert!(bundle.install().unwrap_err().to_string().contains("brew"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_brew_bundle_status_branches() {
        // First check fails: packages are missing outright.
        let bundle = brew_bundle(FakeRunner::new(&[1]));
        assert!(matches!(bundle.status().unwrap(), Status::Missing));

        // First check passes, second fails: installed but out of date.
        let bundle = brew_bundle(FakeRunner::new(&[0, 1]));
        let Status::ConflictUpgrade(conflict) = bundle.status().unwrap() else {
            panic!("expected a conflict when packages need upgrading"); // cov-excl-line
        };
        assert_eq!(conflict.actual, "packages need upgrade");

        // Both checks pass.
        let bundle = brew_bundle(FakeRunner::new(&[0, 0]));
        assert!(matches!(bundle.status().unwrap(), Status::Ok));
        assert_eq!(
            bundle.runner.calls(),
            [
                "brew bundle check --no-upgrade --file=/tmp/Brewfile",
                "brew bundle check --file=/tmp/Brewfile",
            ]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_brew_bundle_install() {
        let bundle = brew_bundle(FakeRunner::new(&[0]));
        bundle.install().unwrap();
        assert_eq!(
            bundle.runner.calls(),
            ["brew bundle install --file=/tmp/Brewfile"]
        );

        let bundle = brew_bundle(FakeRunner::new(&[1]));
        let error = bundle.install().unwrap_err();
        assert!(error.to_string().contains("Failed to install brew bundle"));
    }

    // --- Debug ---

    #[test]
    fn test_debug_defaults_to_ok() {
        let debug = Debug {
            status_fn: None,
            install_fn: None,
        };

        assert_eq!(debug.to_string(), "debug");
        assert!(matches!(debug.status().unwrap(), Status::Ok));

        let error = debug.install().unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Install not implemented for debug assertion")
        );
    }

    #[test]
    fn test_debug_runs_lua_functions() {
        let lua = Lua::new();
        let debug = Debug::from_lua_multi(
            multi(
                &lua,
                r#"return {status = function() return "missing" end, install = function() end}"#,
            ),
            &lua,
        )
        .unwrap();

        assert!(matches!(debug.status().unwrap(), Status::Missing));
        debug.install().unwrap();
    }

    #[test]
    fn test_debug_propagates_lua_failures() {
        let lua = Lua::new();
        let debug = Debug::from_lua_multi(
            multi(
                &lua,
                r#"return {
                    status = function() error("status boom") end,
                    install = function() error("install boom") end,
                }"#,
            ),
            &lua,
        )
        .unwrap();

        let error = debug.status().unwrap_err();
        assert!(error.to_string().contains("Debug status function failed"));

        let error = debug.install().unwrap_err();
        assert!(error.to_string().contains("Debug install function failed"));
    }

    #[test]
    fn test_debug_rejects_non_function_fields() {
        let lua = Lua::new();

        for script in [
            r#"return {status = "missing"}"#,
            r#"return {install = "nothing"}"#,
        ] {
            assert!(Debug::from_lua_multi(multi(&lua, script), &lua).is_err());
        }
    }

    // --- Lua assertion types ---

    fn lua_assertion_type(lua: &Lua, script: &str) -> LuaResult<LuaAssertionType> {
        let value = lua.load(script).eval::<LuaValue>().unwrap();
        LuaAssertionType::from_lua(value, lua)
    }

    #[test]
    fn test_lua_assertion_type_requires_status_and_install() {
        let lua = Lua::new();

        let assertion_type = lua_assertion_type(
            &lua,
            r#"return {status = function() return "ok" end, install = function() end}"#,
        )
        .unwrap();
        assert!(assertion_type.display_fn.is_none());
        assert!(assertion_type.upgrade_fn.is_none());

        assert!(lua_assertion_type(&lua, r#"return {install = function() end}"#).is_err());
    }

    #[test]
    fn test_lua_assertion_type_rejects_non_function_optional_fields() {
        let lua = Lua::new();

        for field in ["display", "upgrade"] {
            let script = format!(
                r#"return {{status = function() return "ok" end, install = function() end, {field} = "oops"}}"#
            );
            assert!(lua_assertion_type(&lua, &script).is_err());
        }
    }

    #[test]
    fn test_lua_assertion_runs_status_and_install() {
        let lua = Lua::new();
        let assertion_type = lua_assertion_type(
            &lua,
            r#"return {
                status = function(a) if a == "yes" then return "ok" else return "missing" end end,
                install = function() end,
            }"#,
        )
        .unwrap();

        let assertion = LuaAssertion::new(
            "custom",
            multi(&lua, r#"return "yes""#),
            assertion_type.clone(),
        );
        assert!(matches!(assertion.status().unwrap(), Status::Ok));
        assertion.install().unwrap();

        let assertion = LuaAssertion::new("custom", multi(&lua, r#"return "no""#), assertion_type);
        assert!(matches!(assertion.status().unwrap(), Status::Missing));
    }

    #[test]
    fn test_lua_assertion_propagates_failures() {
        let lua = Lua::new();
        let assertion_type = lua_assertion_type(
            &lua,
            r#"return {
                status = function() error("status boom") end,
                install = function() error("install boom") end,
            }"#,
        )
        .unwrap();

        let assertion = LuaAssertion::new("custom", LuaMultiValue::new(), assertion_type);
        assert!(assertion.status().is_err());
        assert!(assertion.install().is_err());
    }

    #[test]
    fn test_lua_assertion_upgrade_fn_wins_over_install() {
        let lua = Lua::new();
        let assertion_type = lua_assertion_type(
            &lua,
            r#"return {
                status = function() return "ok" end,
                install = function(a) _G.ran = "install:" .. a end,
                upgrade = function(a) _G.ran = "upgrade:" .. a end,
            }"#,
        )
        .unwrap();

        let assertion = LuaAssertion::new("custom", multi(&lua, r#"return "arg""#), assertion_type);
        assertion.upgrade().unwrap();

        let ran: String = lua.globals().get("ran").unwrap();
        assert_eq!(ran, "upgrade:arg");
    }

    #[test]
    fn test_lua_assertion_upgrade_falls_back_to_install() {
        let lua = Lua::new();
        let assertion_type = lua_assertion_type(
            &lua,
            r#"return {
                status = function() return "ok" end,
                install = function() _G.ran = "install" end,
            }"#,
        )
        .unwrap();

        let assertion = LuaAssertion::new("custom", LuaMultiValue::new(), assertion_type);
        assertion.upgrade().unwrap();

        let ran: String = lua.globals().get("ran").unwrap();
        assert_eq!(ran, "install");
    }

    #[test]
    fn test_lua_assertion_upgrade_propagates_failures() {
        let lua = Lua::new();
        let assertion_type = lua_assertion_type(
            &lua,
            r#"return {
                status = function() return "ok" end,
                install = function() end,
                upgrade = function() error("upgrade boom") end,
            }"#,
        )
        .unwrap();

        let assertion = LuaAssertion::new("custom", LuaMultiValue::new(), assertion_type);
        assert!(assertion.upgrade().is_err());
    }

    #[test]
    fn test_lua_assertion_display_fn_replaces_default() {
        let lua = Lua::new();
        let assertion_type = lua_assertion_type(
            &lua,
            r#"return {
                display = function(a) return "rendered " .. a end,
                status = function() return "ok" end,
                install = function() end,
            }"#,
        )
        .unwrap();

        let assertion = LuaAssertion::new("custom", multi(&lua, r#"return "arg""#), assertion_type);
        assert_eq!(assertion.to_string(), "rendered arg");
    }
}
