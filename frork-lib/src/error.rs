use crate::runtime::Declaration;
use miette::Diagnostic;
use mlua::prelude::*;
use std::sync::Arc;
use thiserror::Error;

/// An error held behind an `Arc` so `FrorkError` can be `Clone` — which it
/// must be to be recovered from a Lua callback, where only a borrowed
/// reference is available. Display and the source chain pass through.
#[derive(Debug, Clone)]
pub struct SharedError(Arc<dyn std::error::Error + Send + Sync + 'static>);

impl SharedError {
    pub fn new(err: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>) -> Self {
        Self(Arc::from(err.into()))
    }
}

impl std::fmt::Display for SharedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for SharedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

/// Everything the library can fail with. Library functions return it
/// directly; the binary converts it to a `miette::Report` for rendering,
/// and errors that cross the Lua boundary keep their source in the
/// corresponding variant.
#[derive(Error, Diagnostic, Debug, Clone)]
pub enum FrorkError {
    #[error("{declaration} is missing an assertion type")]
    #[diagnostic(help("name a provider, like ({declaration} :directory \"~/src\")"))]
    MissingAssertionType { declaration: Declaration },

    #[error("unknown provider :{name}")]
    #[diagnostic(help(
        "the built-ins are symlink, directory, git, brew, brew-bundle, and debug; frork.provide defines more"
    ))]
    UnknownProvider { name: String },

    #[error("Invalid arguments: {0}")]
    InvalidArguments(String),

    #[error("Failed to execute command '{command}'")]
    CommandSpawn {
        command: String,
        #[source]
        source: SharedError,
    },

    #[error("Command '{command}' terminated by signal")]
    CommandSignaled { command: String },

    #[error("Required binary '{bin}' not found in PATH")]
    MissingBinary { bin: String },

    #[error("HOME environment variable not set")]
    MissingHome,

    #[error("Cannot expand '~' in '{path}': only '~' and '~/' are supported")]
    UnsupportedTilde { path: String },

    #[error("Environment variables not found: {}", names.join(", "))]
    UnsetVariables { names: Vec<String> },

    #[error(transparent)]
    Io(SharedError),

    #[error("Failed to create the {provider} assertion")]
    AssertionBuild {
        provider: String,
        #[source]
        source: LuaError,
    },

    #[error("Failed to clone git repository: {stderr}")]
    GitClone { stderr: String },

    #[error("`git {args}` failed in {dir}: {stderr}")]
    GitCommand {
        args: String,
        dir: String,
        stderr: String,
    },

    #[error("Failed to install Homebrew: {stderr}")]
    HomebrewInstall { stderr: String },

    #[error("Failed to install brew bundle: {stderr}")]
    BrewBundleInstall { stderr: String },

    #[error("brew-bundle only supported on Darwin/macOS")]
    MacOsOnly,

    #[error("Failed to {step}")]
    LuaBridge {
        step: &'static str,
        #[source]
        source: LuaError,
    },

    #[error("Failed to execute fennel code")]
    ScriptEval(#[source] LuaError),

    #[error("Failed to execute script {script}")]
    ScriptRun {
        script: String,
        #[source]
        source: LuaError,
    },

    #[error("Debug {hook} function failed")]
    DebugHook {
        hook: &'static str,
        #[source]
        source: LuaError,
    },

    #[error("{hook} not implemented for provider '{provider}'")]
    MissingHook {
        hook: &'static str,
        provider: String,
    },

    #[error("{provider} cannot be removed")]
    #[diagnostic(help("uninstall it by hand, or drop the declaration instead"))]
    RemoveUnsupported { provider: String },

    #[error("refusing to remove {path}: found {found}")]
    #[diagnostic(help("frork only removes what it manages; move or delete it yourself"))]
    RemoveRefused { path: String, found: String },

    #[error("refusing to remove {path}: found {found}")]
    #[diagnostic(help("push or discard the work yourself, then remove it again"))]
    RemoveUnsaved { path: String, found: String },

    #[error("Lua error: {0}")]
    Lua(#[source] LuaError),
}

impl From<std::io::Error> for FrorkError {
    fn from(source: std::io::Error) -> Self {
        FrorkError::Io(SharedError::new(source))
    }
}

impl FrorkError {
    /// Pulls a `FrorkError` back out of the `CallbackError`/`ExternalError`
    /// layers the Lua boundary wraps it in, so it surfaces whole instead of
    /// being flattened into a message.
    pub(crate) fn recover(err: &LuaError) -> Option<Self> {
        match err {
            LuaError::CallbackError { cause, .. } => cause.downcast_ref::<FrorkError>().cloned(),
            _ => None,
        }
    }
}

impl From<LuaError> for FrorkError {
    fn from(err: LuaError) -> Self {
        Self::recover(&err).unwrap_or_else(|| FrorkError::Lua(err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_regular_lua_error_is_kept_as_the_source() {
        let lua = Lua::new();

        let test_fn = lua
            .create_function(|_lua, ()| -> LuaResult<()> {
                Err(LuaError::RuntimeError("regular lua error".to_string()))
            })
            .unwrap();

        let result: Result<(), LuaError> = test_fn.call(());
        let lua_err = result.unwrap_err();
        let frork_err = FrorkError::from(lua_err);

        // The callback error is preserved, source and all, not flattened.
        match frork_err {
            FrorkError::Lua(source) => {
                assert!(source.to_string().contains("regular lua error"));
            }
            _ => panic!("expected a Lua error, got {frork_err:?}"), // cov-excl-line
        }
    }

    #[test]
    fn test_lua_error_outside_callback_keeps_its_shape() {
        let frork_err = FrorkError::from(LuaError::RuntimeError("bare error".to_string()));

        match frork_err {
            FrorkError::Lua(source) => {
                assert_eq!(source.to_string(), "runtime error: bare error");
            }
            _ => panic!("expected a Lua error, got {frork_err:?}"), // cov-excl-line
        }
    }
}
