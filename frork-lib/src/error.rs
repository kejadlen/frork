use miette::Diagnostic;
use mlua::prelude::*;
use thiserror::Error;

#[derive(Error, Diagnostic, Debug, Clone, PartialEq)]
pub enum FrorkError {
    #[error("No operation specified")]
    NoOperation,
    #[error("unknown provider :{name}")]
    UnknownProvider { name: String },
    #[error("Invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("Missing provider")]
    MissingProvider,
    #[error("Lua error: {0}")]
    Lua(String),
    #[error("{provider} cannot be removed")]
    #[diagnostic(help("uninstall it by hand, or drop the declaration instead"))]
    RemoveUnsupported { provider: String },
    #[error("refusing to remove {path}: found {found}")]
    #[diagnostic(help("frork only removes what it manages; move or delete it yourself"))]
    RemoveRefused { path: String, found: String },
    #[error("refusing to remove {path}: found {found}")]
    #[diagnostic(help("push or discard the work yourself, then remove it again"))]
    RemoveUnsaved { path: String, found: String },
    #[error("refusing to update {path}: found {found}")]
    #[diagnostic(help("frork only replaces a symlink; move or delete it yourself"))]
    UpdateRefused { path: String, found: String },
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_regular_lua_error_converted() {
        let lua = Lua::new();

        let test_fn = lua
            .create_function(|_lua, ()| -> LuaResult<()> {
                Err(LuaError::RuntimeError("regular lua error".to_string()))
            })
            .unwrap();

        let result: Result<(), LuaError> = test_fn.call(());
        let lua_err = result.unwrap_err();
        let frork_err = FrorkError::from(lua_err);

        assert_eq!(
            frork_err,
            FrorkError::Lua("Callback error: runtime error: regular lua error".to_string())
        );
    }

    #[test]
    fn test_lua_error_outside_callback_uses_display() {
        let frork_err = FrorkError::from(LuaError::RuntimeError("bare error".to_string()));

        assert_eq!(
            frork_err,
            FrorkError::Lua("runtime error: bare error".to_string())
        );
    }
}
