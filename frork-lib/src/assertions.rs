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

pub trait Provider: std::fmt::Display {
    fn status(&self) -> Result<Status>;
    fn create(&self) -> Result<()>;
    fn update(&self) -> Result<()>;

    // Not yet called — will be used once removal can be declared.
    // cov-excl-start
    #[allow(dead_code)]
    fn remove(&self) -> Result<()> {
        todo!()
    }
    // cov-excl-stop
}

#[derive(Debug)]
pub enum Status {
    Present,
    Absent,
    Drifted(Drift),
}

impl FromLuaMulti for Status {
    fn from_lua_multi(values: LuaMultiValue, lua: &Lua) -> LuaResult<Self> {
        if let Ok((status_str, drift)) = <(String, Drift)>::from_lua_multi(values.clone(), lua) {
            return match status_str.as_str() {
                "drifted" => Ok(Status::Drifted(drift)),
                _ => Err(LuaError::FromLuaConversionError {
                    from: "multivalue",
                    to: "Status".to_string(),
                    message: Some(
                        "String + drift combination only supported for drifted".to_string(),
                    ),
                }),
            };
        }

        if let Ok(status_str) = String::from_lua_multi(values, lua) {
            return match status_str.as_str() {
                "present" => Ok(Status::Present),
                "absent" => Ok(Status::Absent),
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
            message: Some("Expected single string or drifted with table".to_string()),
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct Drift {
    pub expected: String,
    pub actual: String,
}

impl FromLua for Drift {
    fn from_lua(value: LuaValue, lua: &Lua) -> LuaResult<Self> {
        lua.from_value(value)
    }
}

pub trait ProviderFactory {
    fn build(&self, lua: &Lua, args: LuaMultiValue) -> Result<Box<dyn Provider>>;
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

impl<T> ProviderFactory for TypedFactory<T>
where
    T: Provider + FromLuaMulti + 'static,
{
    fn build(&self, lua: &Lua, args: LuaMultiValue) -> Result<Box<dyn Provider>> {
        T::from_lua_multi(args, lua)
            .map(|t| Box::new(t) as Box<dyn Provider>)
            .map_err(|e| miette!("Failed to create assertion: {e}"))
    }
}

// --- Built-in providers ---

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

impl Provider for Symlink {
    fn status(&self) -> Result<Status> {
        // Unlike `Path::exists`, this doesn't follow the link, so a link to a
        // source that doesn't exist yet still counts as present.
        if fs::symlink_metadata(&self.target).is_err() {
            return Ok(Status::Absent);
        }

        if let Ok(link_target) = fs::read_link(&self.target) {
            if link_target == Path::new(&self.source) {
                Ok(Status::Present)
            } else {
                Ok(Status::Drifted(Drift {
                    expected: self.source.to_string(),
                    actual: format!("points to {}", link_target.display()),
                }))
            }
        } else {
            Ok(Status::Drifted(Drift {
                expected: format!("symlink to {}", self.source),
                actual: "not a symlink".to_string(),
            }))
        }
    }

    fn create(&self) -> Result<()> {
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
    fn update(&self) -> Result<()> {
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

impl Provider for Directory {
    fn status(&self) -> Result<Status> {
        let path = Path::new(&self.path);
        if path.is_dir() {
            return Ok(Status::Present);
        }

        // Unlike `Path::exists`, this doesn't follow the link, so a dangling
        // link reads as something in the way rather than nothing there.
        let Ok(metadata) = fs::symlink_metadata(path) else {
            return Ok(Status::Absent);
        };
        Ok(Status::Drifted(Drift {
            expected: "directory".to_string(),
            actual: file_kind(metadata.file_type()).to_string(),
        }))
    }

    fn create(&self) -> Result<()> {
        fs::create_dir_all(&self.path).into_diagnostic()?;
        debug!("created: {}", self);
        Ok(())
    }

    // cov-excl-start
    fn update(&self) -> Result<()> {
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

impl Provider for Git {
    fn status(&self) -> Result<Status> {
        Utils::assert_bin("git")?;

        let dir_path = Path::new(&self.dir);

        if !dir_path.exists() {
            return Ok(Status::Absent);
        }

        if !dir_path.is_dir() {
            return Ok(Status::Drifted(Drift {
                expected: "directory".to_string(),
                actual: "file".to_string(),
            }));
        }

        // An empty directory can still be cloned into, so it reads as
        // not-yet-cloned.
        let mut entries = fs::read_dir(dir_path).into_diagnostic()?;
        if entries.next().is_none() {
            return Ok(Status::Absent);
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
            return Ok(Status::Drifted(Drift {
                expected: format!("git repo with remote {}", self.remote_url),
                actual: "failed to get remote url".to_string(),
            }));
        }

        let current_remote = Utils::chomp(&remote_output);
        if current_remote == self.remote_url {
            Ok(Status::Present)
        } else {
            Ok(Status::Drifted(Drift {
                expected: self.remote_url.clone(),
                actual: format!("current remote: {}", current_remote),
            }))
        }
    }

    fn create(&self) -> Result<()> {
        Utils::assert_bin("git")?;

        let (_output, exit_code) = Utils::sh("git", &["clone", &self.remote_url, &self.dir])?;

        if exit_code != 0 {
            return Err(miette!("Failed to clone git repository"));
        }

        debug!("created: {}", self);
        Ok(())
    }

    // cov-excl-start
    fn update(&self) -> Result<()> {
        todo!()
    }
    // cov-excl-stop
}

/// A seam over shelling out, so providers can be exercised without
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

impl<R: CommandRunner> Provider for Brew<R> {
    fn status(&self) -> Result<Status> {
        if !self.runner.has_bin("brew") {
            return Ok(Status::Absent);
        }

        let (_output, exit_code) = self.runner.run("brew", &["--version"])?;
        if exit_code == 0 {
            Ok(Status::Present)
        } else {
            Ok(Status::Absent)
        }
    }

    // This needs sudo — figure out how to make this work through frork.
    fn create(&self) -> Result<()> {
        let install_args = [
            "-c",
            r#"/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)""#,
        ];
        let (output, exit_code) = self.runner.run("bash", &install_args)?;

        if exit_code != 0 {
            return Err(miette!("Failed to install Homebrew: {}", output));
        }

        debug!("created: {}", self);
        Ok(())
    }

    // cov-excl-start
    fn update(&self) -> Result<()> {
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

impl<R: CommandRunner> Provider for BrewBundle<R> {
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
            return Ok(Status::Absent);
        }

        let check_args = ["bundle", "check", file_arg.as_str()];
        let (_output, exit_code) =
            self.runner
                .run_with_envs("brew", &check_args, &no_auto_update)?;

        if exit_code != 0 {
            return Ok(Status::Drifted(Drift {
                expected: "up-to-date packages".to_string(),
                actual: "packages need upgrade".to_string(),
            }));
        }

        Ok(Status::Present)
    }

    #[cfg(not(target_os = "macos"))]
    fn create(&self) -> Result<()> {
        Err(miette!("brew-bundle only supported on Darwin/macOS"))
    }

    #[cfg(target_os = "macos")]
    fn create(&self) -> Result<()> {
        self.runner.require_bin("brew")?;

        let file_arg = format!("--file={}", self.brewfile);
        let (_output, exit_code) = self
            .runner
            .run("brew", &["bundle", "install", file_arg.as_str()])?;

        if exit_code != 0 {
            return Err(miette!("Failed to install brew bundle"));
        }

        debug!("created: {}", self);
        Ok(())
    }

    // cov-excl-start
    fn update(&self) -> Result<()> {
        todo!()
    }
    // cov-excl-stop
}

pub struct Debug {
    pub status_fn: Option<LuaFunction>,
    pub create_fn: Option<LuaFunction>,
}

impl FromLuaMulti for Debug {
    fn from_lua_multi(args: LuaMultiValue, lua: &Lua) -> LuaResult<Self> {
        let table = LuaTable::from_lua_multi(args, lua)?;
        let status_fn: Option<LuaFunction> = table.get("status")?;
        let create_fn: Option<LuaFunction> = table.get("create")?;
        Ok(Self {
            status_fn,
            create_fn,
        })
    }
}

impl std::fmt::Display for Debug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "debug")
    }
}

impl Provider for Debug {
    fn status(&self) -> Result<Status> {
        if let Some(ref status_fn) = self.status_fn {
            let result = status_fn
                .call::<Status>(LuaMultiValue::new())
                .map_err(|e| miette!("Debug status function failed: {e}"))?;
            Ok(result)
        } else {
            Ok(Status::Present)
        }
    }

    fn create(&self) -> Result<()> {
        info!("debug: creating");
        let create_fn = self
            .create_fn
            .as_ref()
            .ok_or_else(|| miette!("Create not implemented for debug assertion"))?;
        create_fn
            .call::<()>(LuaMultiValue::new())
            .map_err(|e| miette!("Debug create function failed: {e}"))?;
        Ok(())
    }

    // cov-excl-start
    fn update(&self) -> Result<()> {
        todo!()
    }
    // cov-excl-stop
}

// --- Providers defined in Lua ---

#[derive(Clone)]
pub struct LuaProvider {
    pub display_fn: Option<LuaFunction>,
    pub status_fn: LuaFunction,
    pub create_fn: LuaFunction,
    pub update_fn: Option<LuaFunction>,
}

impl FromLua for LuaProvider {
    fn from_lua(value: LuaValue, lua: &Lua) -> LuaResult<Self> {
        let table = LuaTable::from_lua(value, lua)?;

        let display_fn: Option<LuaFunction> = table.get("display")?;
        let status_fn: LuaFunction = table.get("status")?;
        let create_fn: LuaFunction = table.get("create")?;
        let update_fn: Option<LuaFunction> = table.get("update")?;

        Ok(Self {
            display_fn,
            status_fn,
            create_fn,
            update_fn,
        })
    }
}

pub struct LuaAssertion {
    pub name: String,
    pub args: LuaMultiValue,
    pub provider: LuaProvider,
}

impl LuaAssertion {
    pub fn new(name: &str, args: LuaMultiValue, provider: LuaProvider) -> Self {
        Self {
            name: name.to_string(),
            args,
            provider,
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

        if let Some(ref display_fn) = self.provider.display_fn {
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

impl Provider for LuaAssertion {
    fn status(&self) -> Result<Status> {
        let result = self
            .provider
            .status_fn
            .call::<Status>(self.args.clone())
            .map_err(FrorkError::from)?;
        Ok(result)
    }

    fn create(&self) -> Result<()> {
        self.provider
            .create_fn
            .call::<()>(self.args.clone())
            .map_err(FrorkError::from)?;
        debug!("created: {}", self);
        Ok(())
    }

    fn update(&self) -> Result<()> {
        // A missing update function falls back to create — re-running the
        // create is the default way to bring a drifted assertion in line.
        if let Some(update_fn) = self.provider.update_fn.as_ref() {
            update_fn
                .call::<()>(self.args.clone())
                .map_err(FrorkError::from)?;
        } else {
            self.create()?;
        }
        debug!("updated: {}", self);
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

    // --- Status / Drift ---

    fn status_from(lua: &Lua, script: &str) -> LuaResult<Status> {
        Status::from_lua_multi(multi(lua, script), lua)
    }

    #[test]
    fn test_status_from_lua_single_strings() {
        let lua = Lua::new();

        assert!(matches!(
            status_from(&lua, r#"return "present""#),
            Ok(Status::Present)
        ));
        assert!(matches!(
            status_from(&lua, r#"return "absent""#),
            Ok(Status::Absent)
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
                .contains("Expected single string or drifted with table")
        );
    }

    // --- TypedFactory ---

    #[test]
    fn test_typed_factory_creates_assertion() {
        let lua = Lua::new();
        let factory: TypedFactory<Directory> = TypedFactory::default();

        let assertion = factory
            .build(&lua, multi(&lua, r#"return "/tmp""#))
            .unwrap();
        assert_eq!(assertion.to_string(), "directory /tmp");
    }

    #[test]
    fn test_typed_factory_reports_conversion_failure() {
        let lua = Lua::new();
        let factory: TypedFactory<Directory> = TypedFactory::new();

        // Box<dyn Provider> is not Debug, so unwrap_err is unavailable.
        let Err(error) = factory.build(&lua, multi(&lua, r#"return true"#)) else {
            panic!("expected a conversion failure"); // cov-excl-line
        };
        assert!(error.to_string().contains("Failed to create assertion"));
    }

    // --- Symlink ---

    #[test]
    fn test_symlink_status_drifts() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source");
        fs::write(&source, "").unwrap();

        // A link pointing somewhere else drifts.
        let other = dir.path().join("other");
        fs::write(&other, "").unwrap();
        let target = dir.path().join("link");
        std::os::unix::fs::symlink(&other, &target).unwrap();
        let symlink = Symlink {
            target: expanded(&target),
            source: expanded(&source),
        };
        let Status::Drifted(drift) = symlink.status().unwrap() else {
            panic!("expected drift for a wrong-target link"); // cov-excl-line
        };
        assert_eq!(drift.expected, source.display().to_string());
        assert_eq!(drift.actual, format!("points to {}", other.display()));

        // A non-symlink where the link goes drifts too.
        let file = dir.path().join("file-target");
        fs::write(&file, "").unwrap();
        let symlink = Symlink {
            target: expanded(&file),
            source: expanded(&source),
        };
        let Status::Drifted(drift) = symlink.status().unwrap() else {
            panic!("expected drift for a non-symlink target"); // cov-excl-line
        };
        assert_eq!(drift.expected, format!("symlink to {}", source.display()));
        assert_eq!(drift.actual, "not a symlink");
    }

    #[test]
    fn test_symlink_create_failure() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source.txt");
        fs::write(&source, "").unwrap();
        let symlink = Symlink {
            target: expanded(&dir.path().join("missing-parent/link")),
            source: expanded(&source),
        };

        let error = symlink.create().unwrap_err();
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
    fn test_symlink_create_warns_about_a_missing_source() {
        let dir = TempDir::new().unwrap();
        let symlink = Symlink {
            target: expanded(&dir.path().join("link")),
            source: expanded(&dir.path().join("never-created")),
        };

        let warnings = warnings_from(|| symlink.create().unwrap());

        assert!(warnings.contains("source does not exist"), "{warnings}");
        assert!(matches!(symlink.status().unwrap(), Status::Present));
    }

    #[test]
    fn test_symlink_create_resolves_a_relative_source_beside_the_link() {
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

        assert_eq!(warnings_from(|| present.create().unwrap()), "");
        assert!(warnings_from(|| absent.create().unwrap()).contains("source does not exist"));
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

    fn directory_drift(path: &Path) -> Drift {
        let directory = Directory {
            path: expanded(path),
        };
        let Status::Drifted(drift) = directory.status().unwrap() else {
            panic!("expected drift for {path:?}"); // cov-excl-line
        };
        assert_eq!(drift.expected, "directory");
        drift
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

        assert_eq!(directory_drift(&file).actual, "file");
        assert_eq!(directory_drift(&to_file).actual, "symlink");
        assert_eq!(directory_drift(&dangling).actual, "symlink");
        assert_eq!(directory_drift(&fifo).actual, "fifo");
        assert_eq!(directory_drift(&socket).actual, "socket");
        assert_eq!(directory_drift(Path::new("/dev/null")).actual, "device");
    }

    #[test]
    fn test_directory_status_follows_a_link_to_a_directory() {
        let dir = TempDir::new().unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(dir.path(), &link).unwrap();

        let directory = Directory {
            path: expanded(&link),
        };
        assert!(matches!(directory.status().unwrap(), Status::Present));
    }

    #[test]
    fn test_directory_create_failure() {
        let dir = TempDir::new().unwrap();
        let blocker = dir.path().join("blocker");
        fs::write(&blocker, "not a directory").unwrap();

        let directory = Directory {
            path: expanded(&blocker.join("child")),
        };
        assert!(directory.create().is_err());
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
        assert!(matches!(git.status().unwrap(), Status::Absent));

        git.create().unwrap();
        assert!(matches!(git.status().unwrap(), Status::Present));
    }

    #[test]
    fn test_git_status_drifts() {
        let dir = TempDir::new().unwrap();
        let remote_url = bare_origin(dir.path());

        // A file where a directory is expected.
        let file = dir.path().join("a-file");
        fs::write(&file, "content").unwrap();
        let git = Git {
            dir: expanded(&file),
            remote_url: remote_url.clone(),
        };
        let Status::Drifted(drift) = git.status().unwrap() else {
            panic!("expected drift for a non-directory path"); // cov-excl-line
        };
        assert_eq!(drift.expected, "directory");
        assert_eq!(drift.actual, "file");

        // An empty directory counts as absent rather than drifted.
        let empty = dir.path().join("empty");
        fs::create_dir_all(&empty).unwrap();
        let git = Git {
            dir: expanded(&empty),
            remote_url: remote_url.clone(),
        };
        assert!(matches!(git.status().unwrap(), Status::Absent));

        // A non-empty directory that is not a repo has no remote to read.
        let not_a_repo = dir.path().join("not-a-repo");
        fs::create_dir_all(&not_a_repo).unwrap();
        fs::write(not_a_repo.join("file.txt"), "content").unwrap();
        let git = Git {
            dir: expanded(&not_a_repo),
            remote_url: remote_url.clone(),
        };
        let Status::Drifted(drift) = git.status().unwrap() else {
            panic!("expected drift when the remote cannot be read"); // cov-excl-line
        };
        assert_eq!(drift.actual, "failed to get remote url");
    }

    #[test]
    fn test_git_create_failure() {
        let dir = TempDir::new().unwrap();
        let git = Git {
            dir: expanded(&dir.path().join("clone")),
            remote_url: dir.path().join("does-not-exist.git").display().to_string(),
        };

        let error = git.create().unwrap_err();
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

        assert!(matches!(brew.status().unwrap(), Status::Absent));
        assert!(brew.runner.calls().is_empty());
    }

    #[test]
    fn test_brew_status_reflects_version_exit_code() {
        let brew = Brew {
            runner: FakeRunner::new(&[0]),
        };
        assert!(matches!(brew.status().unwrap(), Status::Present));
        assert_eq!(brew.runner.calls(), ["brew --version"]);

        // Present on PATH but not runnable.
        let brew = Brew {
            runner: FakeRunner::new(&[1]),
        };
        assert!(matches!(brew.status().unwrap(), Status::Absent));
    }

    #[test]
    fn test_brew_create() {
        let brew = Brew {
            runner: FakeRunner::new(&[0]),
        };
        brew.create().unwrap();
        assert!(brew.runner.calls()[0].contains("install.sh"));

        let brew = Brew {
            runner: FakeRunner::new(&[1]),
        };
        let error = brew.create().unwrap_err();
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
        assert!(bundle.create().is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_brew_bundle_status_requires_brew() {
        let bundle = brew_bundle(FakeRunner::without_bin("brew"));

        assert!(bundle.status().unwrap_err().to_string().contains("brew"));
        assert!(bundle.create().unwrap_err().to_string().contains("brew"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_brew_bundle_status_branches() {
        // First check fails: packages are absent outright.
        let bundle = brew_bundle(FakeRunner::new(&[1]));
        assert!(matches!(bundle.status().unwrap(), Status::Absent));

        // First check passes, second fails: installed but out of date.
        let bundle = brew_bundle(FakeRunner::new(&[0, 1]));
        let Status::Drifted(drift) = bundle.status().unwrap() else {
            panic!("expected drift when packages are out of date"); // cov-excl-line
        };
        assert_eq!(drift.actual, "packages need upgrade");

        // Both checks pass.
        let bundle = brew_bundle(FakeRunner::new(&[0, 0]));
        assert!(matches!(bundle.status().unwrap(), Status::Present));
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
    fn test_brew_bundle_create() {
        let bundle = brew_bundle(FakeRunner::new(&[0]));
        bundle.create().unwrap();
        assert_eq!(
            bundle.runner.calls(),
            ["brew bundle install --file=/tmp/Brewfile"]
        );

        let bundle = brew_bundle(FakeRunner::new(&[1]));
        let error = bundle.create().unwrap_err();
        assert!(error.to_string().contains("Failed to install brew bundle"));
    }

    // --- Debug ---

    #[test]
    fn test_debug_defaults_to_ok() {
        let debug = Debug {
            status_fn: None,
            create_fn: None,
        };

        assert_eq!(debug.to_string(), "debug");
        assert!(matches!(debug.status().unwrap(), Status::Present));

        let error = debug.create().unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Create not implemented for debug assertion")
        );
    }

    #[test]
    fn test_debug_runs_lua_functions() {
        let lua = Lua::new();
        let debug = Debug::from_lua_multi(
            multi(
                &lua,
                r#"return {status = function() return "absent" end, create = function() end}"#,
            ),
            &lua,
        )
        .unwrap();

        assert!(matches!(debug.status().unwrap(), Status::Absent));
        debug.create().unwrap();
    }

    #[test]
    fn test_debug_propagates_lua_failures() {
        let lua = Lua::new();
        let debug = Debug::from_lua_multi(
            multi(
                &lua,
                r#"return {
                    status = function() error("status boom") end,
                    create = function() error("create boom") end,
                }"#,
            ),
            &lua,
        )
        .unwrap();

        let error = debug.status().unwrap_err();
        assert!(error.to_string().contains("Debug status function failed"));

        let error = debug.create().unwrap_err();
        assert!(error.to_string().contains("Debug create function failed"));
    }

    #[test]
    fn test_debug_rejects_non_function_fields() {
        let lua = Lua::new();

        for script in [
            r#"return {status = "absent"}"#,
            r#"return {create = "nothing"}"#,
        ] {
            assert!(Debug::from_lua_multi(multi(&lua, script), &lua).is_err());
        }
    }

    // --- Lua providers ---

    fn lua_provider(lua: &Lua, script: &str) -> LuaResult<LuaProvider> {
        let value = lua.load(script).eval::<LuaValue>().unwrap();
        LuaProvider::from_lua(value, lua)
    }

    #[test]
    fn test_lua_assertion_type_requires_status_and_create() {
        let lua = Lua::new();

        let provider = lua_provider(
            &lua,
            r#"return {status = function() return "present" end, create = function() end}"#,
        )
        .unwrap();
        assert!(provider.display_fn.is_none());
        assert!(provider.update_fn.is_none());

        assert!(lua_provider(&lua, r#"return {create = function() end}"#).is_err());
    }

    #[test]
    fn test_lua_assertion_type_rejects_non_function_optional_fields() {
        let lua = Lua::new();

        for field in ["display", "update"] {
            let script = format!(
                r#"return {{status = function() return "present" end, create = function() end, {field} = "oops"}}"#
            );
            assert!(lua_provider(&lua, &script).is_err());
        }
    }

    #[test]
    fn test_lua_assertion_runs_status_and_create() {
        let lua = Lua::new();
        let provider = lua_provider(
            &lua,
            r#"return {
                status = function(a) if a == "yes" then return "present" else return "absent" end end,
                create = function() end,
            }"#,
        )
        .unwrap();

        let assertion =
            LuaAssertion::new("custom", multi(&lua, r#"return "yes""#), provider.clone());
        assert!(matches!(assertion.status().unwrap(), Status::Present));
        assertion.create().unwrap();

        let assertion = LuaAssertion::new("custom", multi(&lua, r#"return "no""#), provider);
        assert!(matches!(assertion.status().unwrap(), Status::Absent));
    }

    #[test]
    fn test_lua_assertion_propagates_failures() {
        let lua = Lua::new();
        let provider = lua_provider(
            &lua,
            r#"return {
                status = function() error("status boom") end,
                create = function() error("create boom") end,
            }"#,
        )
        .unwrap();

        let assertion = LuaAssertion::new("custom", LuaMultiValue::new(), provider);
        assert!(assertion.status().is_err());
        assert!(assertion.create().is_err());
    }

    #[test]
    fn test_lua_assertion_update_fn_wins_over_create() {
        let lua = Lua::new();
        let provider = lua_provider(
            &lua,
            r#"return {
                status = function() return "present" end,
                create = function(a) _G.ran = "create:" .. a end,
                update = function(a) _G.ran = "update:" .. a end,
            }"#,
        )
        .unwrap();

        let assertion = LuaAssertion::new("custom", multi(&lua, r#"return "arg""#), provider);
        assertion.update().unwrap();

        let ran: String = lua.globals().get("ran").unwrap();
        assert_eq!(ran, "update:arg");
    }

    #[test]
    fn test_lua_assertion_update_falls_back_to_create() {
        let lua = Lua::new();
        let provider = lua_provider(
            &lua,
            r#"return {
                status = function() return "present" end,
                create = function() _G.ran = "create" end,
            }"#,
        )
        .unwrap();

        let assertion = LuaAssertion::new("custom", LuaMultiValue::new(), provider);
        assertion.update().unwrap();

        let ran: String = lua.globals().get("ran").unwrap();
        assert_eq!(ran, "create");
    }

    #[test]
    fn test_lua_assertion_update_propagates_failures() {
        let lua = Lua::new();
        let provider = lua_provider(
            &lua,
            r#"return {
                status = function() return "present" end,
                create = function() end,
                update = function() error("update boom") end,
            }"#,
        )
        .unwrap();

        let assertion = LuaAssertion::new("custom", LuaMultiValue::new(), provider);
        assert!(assertion.update().is_err());
    }

    #[test]
    fn test_lua_assertion_display_fn_replaces_default() {
        let lua = Lua::new();
        let provider = lua_provider(
            &lua,
            r#"return {
                display = function(a) return "rendered " .. a end,
                status = function() return "present" end,
                create = function() end,
            }"#,
        )
        .unwrap();

        let assertion = LuaAssertion::new("custom", multi(&lua, r#"return "arg""#), provider);
        assert_eq!(assertion.to_string(), "rendered arg");
    }
}
