use mlua::prelude::*;
use thiserror::Error;

#[derive(Error, Debug, Clone)]
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

// TODO does this actually work the way I think it does? write a test to find out
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

