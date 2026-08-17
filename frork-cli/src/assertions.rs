use fs_err as fs;
use miette::IntoDiagnostic as _;
use miette::Result;
use miette::miette;
use mlua::prelude::*;
use serde::Deserialize;
use std::path::Path;
use tracing::debug;
use tracing::error;
use tracing::info;

use crate::error::FrorkError;
use crate::utils::ExpandedPath;
use crate::utils::Utils;

pub trait AssertionType: std::fmt::Display {
    fn status(&self) -> Result<Status>;
    fn install(&self) -> Result<()>;

    // Neither is implemented for any assertion type yet. They default here so
    // the stubs live in one place rather than being repeated per impl. Delete
    // the markers below along with the todo! when implementing either.
    // cov-excl-start
    fn upgrade(&self) -> Result<()> {
        todo!()
    }

    // Not yet called — will be used when `frork remove` is implemented.
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
        // Try two values: string and conflict for conflict-upgrade.
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

        // Try single string.
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
                todo!("{self} points to the wrong target");
            }
        } else {
            todo!("{self} exists but is not a symlink");
        }
    }

    fn install(&self) -> Result<()> {
        use std::os::unix::fs;
        fs::symlink(&self.source, &self.target)
            .map_err(|e| miette!("Failed to create symlink: {e}"))?;
        debug!("created: {}", self);
        Ok(())
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
            todo!("{self} exists but is not a directory");
        } else {
            Ok(Status::Missing)
        }
    }

    fn install(&self) -> Result<()> {
        fs::create_dir_all(&self.path).into_diagnostic()?;
        debug!("created: {}", self);
        Ok(())
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

        // Check if the directory is empty.
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
        // Early return if brew command is not in PATH.
        if Utils::assert_bin("brew").is_err() {
            return Ok(Status::Missing);
        }

        let (_output, exit_code) = Utils::sh("brew", &["--version"])?;
        if exit_code == 0 {
            Ok(Status::Ok)
        } else {
            Ok(Status::Missing)
        }
    }

    // This needs sudo — figure out how to make this work through frork.
    fn install(&self) -> Result<()> {
        let (output, exit_code) = Utils::sh(
            "bash",
            &[
                "-c",
                r#"/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)""#,
            ],
        )?;

        if exit_code != 0 {
            return Err(miette!("Failed to install Homebrew: {}", output));
        }

        debug!("installed: {}", self);
        Ok(())
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
    #[cfg(not(target_os = "macos"))]
    fn status(&self) -> Result<Status> {
        Err(miette!("brew-bundle only supported on Darwin/macOS"))
    }

    #[cfg(target_os = "macos")]
    fn status(&self) -> Result<Status> {
        Utils::assert_bin("brew")?;

        // First check: brew bundle check --no-upgrade.
        let file_arg = format!("--file={}", self.brewfile);
        let (_output, exit_code) = Utils::sh_with_envs(
            "brew",
            &["bundle", "check", "--no-upgrade", &file_arg],
            &[("HOMEBREW_NO_AUTO_UPDATE", "true")],
        )?;

        if exit_code != 0 {
            return Ok(Status::Missing);
        }

        // Second check: brew bundle check (without --no-upgrade).
        let file_arg = format!("--file={}", self.brewfile);
        let (_output, exit_code) = Utils::sh_with_envs(
            "brew",
            &["bundle", "check", &file_arg],
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

    #[cfg(not(target_os = "macos"))]
    fn install(&self) -> Result<()> {
        Err(miette!("brew-bundle only supported on Darwin/macOS"))
    }

    #[cfg(target_os = "macos")]
    fn install(&self) -> Result<()> {
        Utils::assert_bin("brew")?;

        let file_arg = format!("--file={}", self.brewfile);
        let (_output, exit_code) = Utils::sh("brew", &["bundle", "install", &file_arg])?;

        if exit_code != 0 {
            return Err(miette!("Failed to install brew bundle"));
        }

        debug!("installed: {}", self);
        Ok(())
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
}

// --- Lua custom assertion types ---

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn multi(lua: &Lua, script: &str) -> LuaMultiValue {
        lua.load(script).eval::<LuaMultiValue>().unwrap()
    }

    fn expanded(path: &Path) -> ExpandedPath {
        ExpandedPath::try_from(path.to_str().unwrap()).unwrap()
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
    fn test_status_from_lua_conflict_upgrade() {
        let lua = Lua::new();

        let status = status_from(
            &lua,
            r#"return "conflict-upgrade", {expected = "a", actual = "b"}"#,
        )
        .unwrap();
        let Status::ConflictUpgrade(conflict) = status else {
            panic!("expected a conflict-upgrade status"); // cov-excl-line
        };
        assert_eq!(conflict.expected, "a");
        assert_eq!(conflict.actual, "b");
    }

    #[test]
    fn test_status_from_lua_rejects_other_strings_with_conflict() {
        let lua = Lua::new();

        let err = status_from(&lua, r#"return "ok", {expected = "a", actual = "b"}"#).unwrap_err();
        assert!(
            err.to_string()
                .contains("only supported for conflict-upgrade")
        );
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
    fn test_symlink_lifecycle() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source.txt");
        fs::write(&source, "content").unwrap();
        let target = dir.path().join("link");

        let symlink = Symlink {
            target: expanded(&target),
            source: expanded(&source),
        };

        assert_eq!(
            symlink.to_string(),
            format!("symlink {} {}", target.display(), source.display())
        );
        assert!(matches!(symlink.status().unwrap(), Status::Missing));

        symlink.install().unwrap();
        assert!(matches!(symlink.status().unwrap(), Status::Ok));
    }

    #[test]
    fn test_symlink_install_failure() {
        let dir = TempDir::new().unwrap();
        let symlink = Symlink {
            target: expanded(&dir.path().join("missing-parent/link")),
            source: expanded(&dir.path().join("source.txt")),
        };

        let error = symlink.install().unwrap_err();
        assert!(error.to_string().contains("Failed to create symlink"));
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

    #[test]
    fn test_directory_lifecycle() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/deep");
        let directory = Directory {
            path: expanded(&path),
        };

        assert_eq!(
            directory.to_string(),
            format!("directory {}", path.display())
        );
        assert!(matches!(directory.status().unwrap(), Status::Missing));

        directory.install().unwrap();
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
    fn test_git_status_wrong_remote() {
        let dir = TempDir::new().unwrap();
        let remote_url = bare_origin(dir.path());
        let clone = dir.path().join("clone");

        Git {
            dir: expanded(&clone),
            remote_url: remote_url.clone(),
        }
        .install()
        .unwrap();

        let git = Git {
            dir: expanded(&clone),
            remote_url: "https://example.com/other.git".to_string(),
        };
        let Status::ConflictUpgrade(conflict) = git.status().unwrap() else {
            panic!("expected a conflict for a mismatched remote"); // cov-excl-line
        };
        assert_eq!(conflict.expected, "https://example.com/other.git");
        assert!(conflict.actual.contains(&remote_url));
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

    // --- Brew ---

    #[test]
    fn test_brew_display_and_from_lua() {
        let lua = Lua::new();
        let brew = Brew::from_lua_multi(LuaMultiValue::new(), &lua).unwrap();

        assert_eq!(brew.to_string(), "brew");
    }

    #[test]
    fn test_brew_bundle_display_and_from_lua() {
        let lua = Lua::new();
        let bundle =
            BrewBundle::from_lua_multi(multi(&lua, r#"return "/tmp/Brewfile""#), &lua).unwrap();

        assert_eq!(bundle.to_string(), "brew-bundle /tmp/Brewfile");
        assert_eq!(bundle.brewfile.as_str(), "/tmp/Brewfile");
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

        assert!(lua_assertion_type(&lua, r#"return {install = function() end}"#).is_err());
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
    fn test_lua_assertion_display_without_display_fn() {
        let lua = Lua::new();
        let assertion_type = lua_assertion_type(
            &lua,
            r#"return {status = function() return "ok" end, install = function() end}"#,
        )
        .unwrap();

        let assertion =
            LuaAssertion::new("custom", multi(&lua, r#"return "a", "b""#), assertion_type);
        assert_eq!(assertion.to_string(), "custom a b");
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

    #[test]
    fn test_lua_assertion_display_fn_failure_falls_back() {
        let lua = Lua::new();
        let assertion_type = lua_assertion_type(
            &lua,
            r#"return {
                display = function() error("display boom") end,
                status = function() return "ok" end,
                install = function() end,
            }"#,
        )
        .unwrap();

        let assertion = LuaAssertion::new("custom", multi(&lua, r#"return "arg""#), assertion_type);
        assert_eq!(assertion.to_string(), "custom arg");
    }
}
