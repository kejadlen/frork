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
/// directly; the binary converts it to a `miette::Report` for rendering.
///
/// Leaf variants say what went wrong. Only the context variants say where:
/// the script (`ScriptEval`/`ScriptRun`), the declaration (`Declaration`),
/// building an assertion (`AssertionBuild`), a Fennel provider's hook
/// (`ProviderHook`), and the Lua plumbing (`LuaBridge`). Each holds a
/// `Cause`, so a `FrorkError` raised on the far side of the Lua boundary
/// keeps its type, and forwards its diagnostic, so the innermost error's
/// help shows at the top.
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

    #[error("{function} requires at least a command")]
    MissingCommand { function: &'static str },

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

    #[error("Failed to {step}")]
    Prompt {
        step: &'static str,
        #[source]
        source: SharedError,
    },

    #[error("{}{declaration}{}", at(location), subject_suffix(subject))]
    #[diagnostic(forward(source))]
    Declaration {
        declaration: Declaration,
        /// The assertion, or its provider name if it was never built.
        subject: Option<String>,
        /// Where the script declared it, as `file:line`.
        location: Option<String>,
        #[source]
        source: Cause,
    },

    #[error("Failed to create the {provider} assertion")]
    #[diagnostic(forward(source))]
    AssertionBuild {
        provider: String,
        #[source]
        source: Cause,
    },

    #[error("{hook} hook of provider :{provider} failed")]
    #[diagnostic(forward(source))]
    ProviderHook {
        provider: String,
        hook: &'static str,
        #[source]
        source: Cause,
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
    #[diagnostic(forward(source))]
    LuaBridge {
        step: &'static str,
        #[source]
        source: Cause,
    },

    #[error("Failed to execute fennel code")]
    #[diagnostic(forward(0))]
    ScriptEval(#[source] Cause),

    #[error("Failed to execute script {script}")]
    #[diagnostic(forward(source))]
    ScriptRun {
        script: String,
        #[source]
        source: Cause,
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
}

fn at(location: &Option<String>) -> String {
    location
        .as_ref()
        .map(|location| format!("{location}: "))
        .unwrap_or_default()
}

fn subject_suffix(subject: &Option<String>) -> String {
    subject
        .as_ref()
        .map(|subject| format!(" {subject}"))
        .unwrap_or_default()
}

impl From<std::io::Error> for FrorkError {
    fn from(source: std::io::Error) -> Self {
        FrorkError::Io(SharedError::new(source))
    }
}

impl FrorkError {
    /// Converts this error for raising into Lua. Lua code sees the whole
    /// chain and any help as one message, and a Rust caller on the other
    /// side gets this `FrorkError` back through `Cause::from`.
    pub fn into_lua_error(self) -> LuaError {
        LuaError::external(Raised(self))
    }

    /// The whole cause chain on one line, joined with `: `, then any help
    /// on the next: everything miette would render, as plain text.
    pub fn full_message(&self) -> String {
        let mut message = self.to_string();
        let mut source = std::error::Error::source(self);
        while let Some(err) = source {
            message.push_str(": ");
            message.push_str(&err.to_string());
            source = err.source();
        }
        if let Some(help) = self.help() {
            message.push_str("\nhelp: ");
            message.push_str(&help.to_string());
        }
        message
    }

    /// The innermost `FrorkError`, past every context layer: the error that
    /// says what actually went wrong.
    pub fn root(&self) -> &FrorkError {
        match self.nested() {
            Some(inner) => inner.root(),
            None => self,
        }
    }

    fn nested(&self) -> Option<&FrorkError> {
        match self {
            FrorkError::Declaration { source, .. }
            | FrorkError::AssertionBuild { source, .. }
            | FrorkError::ProviderHook { source, .. }
            | FrorkError::LuaBridge { source, .. }
            | FrorkError::ScriptEval(source)
            | FrorkError::ScriptRun { source, .. } => source.frork(),
            _ => None,
        }
    }
}

/// What a context variant wraps: a `FrorkError` recovered from the far side
/// of the Lua boundary, or the Lua error itself when there is none. It is
/// transparent, so the inner error's message, help, and causes render as if
/// it were the source directly.
#[derive(Debug, Clone)]
pub enum Cause {
    Frork(Box<FrorkError>),
    // Boxed, like the FrorkError, to keep every FrorkError small.
    Lua(Box<LuaError>),
}

impl Cause {
    pub fn frork(&self) -> Option<&FrorkError> {
        match self {
            Cause::Frork(err) => Some(err),
            Cause::Lua(_) => None,
        }
    }

    /// Walks the `CallbackError`/`WithContext` layers mlua wraps an error
    /// in, however deep, to the `FrorkError` at the bottom.
    fn recover(err: &LuaError) -> Option<FrorkError> {
        match err {
            LuaError::CallbackError { cause, .. } | LuaError::WithContext { cause, .. } => {
                Self::recover(cause)
            }
            LuaError::ExternalError(external) => {
                let raised = external.downcast_ref::<Raised>().map(|raised| &raised.0);
                raised
                    .or_else(|| external.downcast_ref::<FrorkError>())
                    .cloned()
            }
            _ => None,
        }
    }
}

impl From<FrorkError> for Cause {
    fn from(err: FrorkError) -> Self {
        Cause::Frork(Box::new(err))
    }
}

impl From<LuaError> for Cause {
    fn from(err: LuaError) -> Self {
        match Self::recover(&err) {
            Some(frork) => frork.into(),
            None => Cause::Lua(Box::new(err)),
        }
    }
}

impl std::fmt::Display for Cause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Cause::Frork(err) => err.fmt(f),
            Cause::Lua(err) => match err.as_ref() {
                LuaError::RuntimeError(message) => f.write_str(trim_traceback(message)),
                err => err.fmt(f),
            },
        }
    }
}

/// A Fennel parse or compile error already says where the script went
/// wrong; the traceback after it only walks the compiler's internals.
fn trim_traceback(message: &str) -> &str {
    let is_fennel_error = message.contains("Parse error:") || message.contains("Compile error:");
    match message.split_once("\nstack traceback:") {
        Some((head, _)) if is_fennel_error => head.trim_end(),
        _ => message,
    }
}

impl std::error::Error for Cause {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Cause::Frork(err) => err.source(),
            Cause::Lua(err) => err.source(),
        }
    }
}

/// Only help is forwarded: no variant sets a code, URL, or label yet, and
/// the context variants forward theirs, so help is all that reaches here.
impl Diagnostic for Cause {
    fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        self.frork().and_then(Diagnostic::help)
    }
}

/// A `FrorkError` on its way through Lua. Lua only ever sees a message, so
/// this one carries the whole chain and every help line.
#[derive(Debug)]
struct Raised(FrorkError);

impl std::fmt::Display for Raised {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.full_message())
    }
}

impl std::error::Error for Raised {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    fn raising(lua: &Lua, error: LuaError) -> LuaFunction {
        lua.create_function(move |_lua, ()| -> LuaResult<()> { Err(error.clone()) })
            .unwrap()
    }

    fn unknown(name: &str) -> FrorkError {
        FrorkError::UnknownProvider {
            name: name.to_string(),
        }
    }

    fn recovered(cause: &Cause) -> &FrorkError {
        cause.frork().expect("a recovered FrorkError")
    }

    #[test]
    fn test_a_regular_lua_error_is_kept_as_the_cause() {
        let lua = Lua::new();
        let error = LuaError::RuntimeError("regular lua error".to_string());

        let caught = raising(&lua, error).call::<()>(()).unwrap_err();
        let cause = Cause::from(caught);

        let Cause::Lua(source) = &cause else {
            panic!("expected a Lua cause, got {cause:?}"); // cov-excl-line
        };
        assert!(source.to_string().contains("regular lua error"));
        assert!(cause.frork().is_none());
    }

    #[test]
    fn test_a_lua_error_outside_a_callback_keeps_its_shape() {
        let cause = Cause::from(LuaError::RuntimeError("bare error".to_string()));

        assert_eq!(cause.to_string(), "bare error");
        assert!(cause.source().is_none());
        assert!(cause.help().is_none());
    }

    #[test]
    fn test_a_raised_error_is_recovered_whole() {
        let lua = Lua::new();
        let error = unknown("raised").into_lua_error();

        let caught = raising(&lua, error).call::<()>(()).unwrap_err();
        let cause = Cause::from(caught);

        assert!(
            matches!(recovered(&cause), FrorkError::UnknownProvider { name } if name == "raised")
        );
    }

    #[test]
    fn test_a_plain_external_error_is_recovered_too() {
        let lua = Lua::new();
        let error = LuaError::external(unknown("plain"));

        let caught = raising(&lua, error).call::<()>(()).unwrap_err();
        let cause = Cause::from(caught);

        assert!(
            matches!(recovered(&cause), FrorkError::UnknownProvider { name } if name == "plain")
        );
    }

    #[test]
    fn test_an_unrelated_external_error_is_not_recovered() {
        let error = LuaError::external(std::io::Error::other("elsewhere"));

        assert!(Cause::from(error).frork().is_none());
    }

    #[test]
    fn test_recovery_reaches_through_nested_callbacks() {
        let lua = Lua::new();
        let inner = raising(&lua, unknown("deep").into_lua_error());
        // A Rust callback that calls Lua and passes the failure on with `?`
        // stacks a second CallbackError around the first.
        let outer = lua
            .create_function(move |_lua, ()| inner.call::<()>(()))
            .unwrap();
        let caller: LuaFunction = lua.load("return function(f) f() end").eval().unwrap();

        let caught = caller.call::<()>(outer).unwrap_err();
        let LuaError::CallbackError { cause, .. } = &caught else {
            panic!("expected a callback error, got {caught:?}"); // cov-excl-line
        };
        assert!(matches!(**cause, LuaError::CallbackError { .. }));

        let cause = Cause::from(caught);
        assert!(
            matches!(recovered(&cause), FrorkError::UnknownProvider { name } if name == "deep")
        );
    }

    #[test]
    fn test_recovery_reaches_through_added_context() {
        let error = unknown("context").into_lua_error().context("while testing");

        let cause = Cause::from(error);
        assert!(
            matches!(recovered(&cause), FrorkError::UnknownProvider { name } if name == "context")
        );
    }

    #[test]
    fn test_a_recovered_cause_is_transparent() {
        let source = std::io::Error::other("disk on fire");
        let error = FrorkError::CommandSpawn {
            command: "ls".to_string(),
            source: SharedError::new(source),
        };
        let cause = Cause::from(error);

        assert_eq!(cause.to_string(), "Failed to execute command 'ls'");
        assert_eq!(cause.source().unwrap().to_string(), "disk on fire");
        assert!(cause.help().is_none());

        let cause = Cause::from(unknown("helped"));
        assert!(cause.help().unwrap().to_string().contains("frork.provide"));
    }

    #[test]
    fn test_a_fennel_parse_error_drops_the_compiler_traceback() {
        let message = "eval:1:14: Parse error: expected closing delimiter )\n\n(ok\nstack traceback:\n\t[C]: in ?";
        let cause = Cause::from(LuaError::RuntimeError(message.to_string()));

        assert_eq!(
            cause.to_string(),
            "eval:1:14: Parse error: expected closing delimiter )\n\n(ok"
        );

        let message =
            "eval:1:0: Compile error: unknown identifier: frork\nstack traceback:\n\t[C]: in ?";
        let cause = Cause::from(LuaError::RuntimeError(message.to_string()));
        assert_eq!(
            cause.to_string(),
            "eval:1:0: Compile error: unknown identifier: frork"
        );
    }

    #[test]
    fn test_a_runtime_error_keeps_its_traceback() {
        // The script's own traceback, unlike the compiler's, says where it
        // failed.
        let message = "main.fnl:2: boom\nstack traceback:\n\tmain.fnl:2: in main chunk";
        let cause = Cause::from(LuaError::RuntimeError(message.to_string()));

        assert_eq!(cause.to_string(), message);
    }

    #[test]
    fn test_a_fennel_error_without_a_traceback_is_untouched() {
        let message = "eval:1:14: Parse error: expected closing delimiter )";
        let cause = Cause::from(LuaError::RuntimeError(message.to_string()));

        assert_eq!(cause.to_string(), message);
    }

    #[test]
    fn test_a_non_runtime_lua_error_displays_as_itself() {
        let error = LuaError::FromLuaConversionError {
            from: "string",
            to: "Status".to_string(),
            message: None,
        };
        let expected = error.to_string();

        assert_eq!(Cause::from(error).to_string(), expected);
    }

    fn declared(location: Option<&str>, subject: Option<&str>) -> FrorkError {
        FrorkError::Declaration {
            declaration: Declaration::No,
            subject: subject.map(str::to_string),
            location: location.map(str::to_string),
            source: unknown("x").into(),
        }
    }

    #[test]
    fn test_a_declaration_names_where_and_what() {
        let error = declared(Some("main.fnl:3"), Some("symlink /a /b"));
        assert_eq!(error.to_string(), "main.fnl:3: no symlink /a /b");

        assert_eq!(declared(None, Some("x")).to_string(), "no x");
        assert_eq!(declared(Some("eval:1"), None).to_string(), "eval:1: no");
        assert_eq!(declared(None, None).to_string(), "no");
    }

    #[test]
    fn test_context_forwards_the_innermost_help() {
        let error = FrorkError::ScriptRun {
            script: "main.fnl".to_string(),
            source: declared(Some("main.fnl:3"), Some("x")).into(),
        };

        let help = error.help().unwrap().to_string();
        assert!(help.contains("frork.provide"));
    }

    #[test]
    fn test_full_message_joins_the_chain_and_appends_help() {
        let error = FrorkError::ScriptRun {
            script: "main.fnl".to_string(),
            source: declared(Some("main.fnl:3"), Some("x")).into(),
        };

        assert_eq!(
            error.full_message(),
            "Failed to execute script main.fnl: main.fnl:3: no x: unknown provider :x\nhelp: \
             the built-ins are symlink, directory, git, brew, brew-bundle, and debug; \
             frork.provide defines more"
        );
    }

    #[test]
    fn test_full_message_without_help_is_just_the_chain() {
        let error = FrorkError::CommandSpawn {
            command: "ls".to_string(),
            source: SharedError::new(std::io::Error::other("disk on fire")),
        };

        assert_eq!(
            error.full_message(),
            "Failed to execute command 'ls': disk on fire"
        );
    }

    #[test]
    fn test_lua_sees_the_full_message() {
        let error = declared(Some("main.fnl:3"), Some("x"));
        let expected = error.full_message();

        let LuaError::ExternalError(raised) = error.into_lua_error() else {
            panic!("expected an external error"); // cov-excl-line
        };
        assert_eq!(raised.to_string(), expected);
    }

    #[test]
    fn test_root_skips_every_context_layer() {
        let lua_cause = || Cause::from(LuaError::RuntimeError("lua".to_string()));
        let layers = [
            FrorkError::Declaration {
                declaration: Declaration::Ok,
                subject: None,
                location: None,
                source: unknown("root").into(),
            },
            FrorkError::AssertionBuild {
                provider: "p".to_string(),
                source: unknown("root").into(),
            },
            FrorkError::ProviderHook {
                provider: "p".to_string(),
                hook: "status",
                source: unknown("root").into(),
            },
            FrorkError::LuaBridge {
                step: "step",
                source: unknown("root").into(),
            },
            FrorkError::ScriptEval(unknown("root").into()),
            FrorkError::ScriptRun {
                script: "s".to_string(),
                source: unknown("root").into(),
            },
        ];
        for layer in layers {
            let nested = FrorkError::ScriptEval(layer.into());
            assert!(
                matches!(nested.root(), FrorkError::UnknownProvider { name } if name == "root"),
                "through {nested:?}"
            );
        }

        // A Lua cause has no FrorkError under it, so the context is the root.
        let hook = FrorkError::ScriptEval(lua_cause());
        assert!(matches!(hook.root(), FrorkError::ScriptEval(_)));
        assert!(matches!(
            unknown("leaf").root(),
            FrorkError::UnknownProvider { .. }
        ));
    }
}
