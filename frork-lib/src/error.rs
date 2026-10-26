use miette::Diagnostic;
use mlua::prelude::*;
use thiserror::Error;

#[derive(Error, Diagnostic, Debug, Clone, PartialEq)]
pub enum FrorkError {
    #[error("No operation specified")]
    NoOperation,
    #[error("Unknown assertion type: {assertion_type}")]
    UnknownAssertionType { assertion_type: String },
    #[error("Invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("Missing assertion type")]
    MissingAssertionType,
    #[error("Lua error: {0}")]
    Lua(String),
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
    fn test_frork_error_preserved_through_lua_callback() {
        let lua = Lua::new();

        // Create a Lua function that returns a FrorkError wrapped in LuaError::external
        let test_fn = lua
            .create_function(|_lua, ()| -> LuaResult<()> {
                Err(LuaError::external(FrorkError::InvalidArguments(
                    "test error message".to_string(),
                )))
            })
            .unwrap();

        // Call the function and convert the resulting LuaError to FrorkError
        let result: Result<(), LuaError> = test_fn.call(());
        let lua_err = result.unwrap_err();
        let frork_err = FrorkError::from(lua_err);

        // Check that the original FrorkError is preserved
        assert_eq!(
            frork_err,
            FrorkError::InvalidArguments("test error message".to_string())
        );
    }

    #[test]
    fn test_regular_lua_error_converted() {
        let lua = Lua::new();

        // Create a Lua function that returns a regular Lua error
        let test_fn = lua
            .create_function(|_lua, ()| -> LuaResult<()> {
                Err(LuaError::RuntimeError("regular lua error".to_string()))
            })
            .unwrap();

        // Call the function and convert the resulting LuaError to FrorkError
        let result: Result<(), LuaError> = test_fn.call(());
        let lua_err = result.unwrap_err();
        let frork_err = FrorkError::from(lua_err);

        // Check that it's converted to FrorkError::Lua (wrapped as callback error)
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
