use std::collections::HashMap;

use miette::Result;
use mlua::prelude::*;

use crate::assertions::AssertionType;
use crate::assertions::AssertionTypeFactory;
use crate::assertions::Brew;
use crate::assertions::BrewBundle;
use crate::assertions::Debug;
use crate::assertions::Directory;
use crate::assertions::Git;
use crate::assertions::LuaAssertion;
use crate::assertions::LuaAssertionType;
use crate::assertions::Symlink;
use crate::assertions::TypedFactory;
use crate::error::FrorkError;

/// Dispatches an assertion type name to the factory that builds it. Types
/// registered from Fennel shadow the built-ins.
#[derive(Default)]
pub struct Registry {
    lua_assertion_types: HashMap<String, LuaAssertionType>,
}

impl Registry {
    pub fn register(&mut self, name: &str, lua_assertion_type: LuaAssertionType) {
        self.lua_assertion_types
            .insert(name.to_string(), lua_assertion_type);
    }

    pub fn get_factory(&self, assertion_type: &str) -> Result<Box<dyn AssertionTypeFactory>> {
        // Check Lua assertions first.
        if let Some(lua_assertion) = self.lua_assertion_types.get(assertion_type) {
            return Ok(Box::new(LuaAssertionFactory {
                assertion_type: assertion_type.to_string(),
                lua_assertion_type: lua_assertion.clone(),
            }));
        }

        // Return factory for built-in types.
        match assertion_type {
            "brew" => Ok(Box::new(TypedFactory::<Brew>::new())),
            "brew-bundle" => Ok(Box::new(TypedFactory::<BrewBundle>::new())),
            "debug" => Ok(Box::new(TypedFactory::<Debug>::new())),
            "directory" => Ok(Box::new(TypedFactory::<Directory>::new())),
            "git" => Ok(Box::new(TypedFactory::<Git>::new())),
            "symlink" => Ok(Box::new(TypedFactory::<Symlink>::new())),
            _ => Err(FrorkError::UnknownAssertionType {
                assertion_type: assertion_type.to_string(),
            }
            .into()),
        }
    }
}

struct LuaAssertionFactory {
    assertion_type: String,
    lua_assertion_type: LuaAssertionType,
}

impl AssertionTypeFactory for LuaAssertionFactory {
    fn create(&self, _lua: &Lua, args: LuaMultiValue) -> Result<Box<dyn AssertionType>> {
        Ok(Box::new(LuaAssertion::new(
            &self.assertion_type,
            args,
            self.lua_assertion_type.clone(),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua_assertion_type(lua: &Lua) -> LuaAssertionType {
        let value = lua
            .load(r#"return {status = function() return "ok" end, install = function() end}"#)
            .eval::<LuaValue>()
            .unwrap();
        LuaAssertionType::from_lua(value, lua).unwrap()
    }

    /// Every built-in name resolves to a factory that builds a working
    /// assertion, checked through the factory rather than by name alone.
    #[test]
    fn test_built_in_types_resolve() {
        let lua = Lua::new();
        let registry = Registry::default();

        let cases = [
            ("brew", LuaMultiValue::new(), "brew"),
            (
                "brew-bundle",
                lua.load(r#"return "/tmp/Brewfile""#).eval().unwrap(),
                "brew-bundle /tmp/Brewfile",
            ),
            ("debug", lua.load(r#"return {}"#).eval().unwrap(), "debug"),
            (
                "directory",
                lua.load(r#"return "/tmp""#).eval().unwrap(),
                "directory /tmp",
            ),
            (
                "git",
                lua.load(r#"return "/tmp/r", "https://example.com/r.git""#)
                    .eval()
                    .unwrap(),
                "git /tmp/r https://example.com/r.git",
            ),
            (
                "symlink",
                lua.load(r#"return "/tmp/link", "/tmp/source""#)
                    .eval()
                    .unwrap(),
                "symlink /tmp/link /tmp/source",
            ),
        ];

        for (name, args, expected) in cases {
            let factory = registry
                .get_factory(name)
                .unwrap_or_else(|e| panic!("{name} did not resolve: {e}"));
            let assertion = factory
                .create(&lua, args)
                .unwrap_or_else(|e| panic!("{name} failed to build: {e}"));
            assert_eq!(assertion.to_string(), expected, "for type {name}");
        }
    }

    #[test]
    fn test_registered_types_are_dispatched() {
        let lua = Lua::new();
        let mut registry = Registry::default();
        registry.register("custom", lua_assertion_type(&lua));

        let factory = registry.get_factory("custom").unwrap();
        let assertion = factory
            .create(&lua, lua.load(r#"return "arg""#).eval().unwrap())
            .unwrap();

        assert_eq!(assertion.to_string(), "custom arg");
    }

    #[test]
    fn test_re_registering_replaces_the_previous_type() {
        let lua = Lua::new();
        let mut registry = Registry::default();

        registry.register("custom", lua_assertion_type(&lua));
        let replacement = lua
            .load(
                r#"return {
                    display = function() return "replaced" end,
                    status = function() return "ok" end,
                    install = function() end,
                }"#,
            )
            .eval::<LuaValue>()
            .unwrap();
        registry.register(
            "custom",
            LuaAssertionType::from_lua(replacement, &lua).unwrap(),
        );

        let factory = registry.get_factory("custom").unwrap();
        let assertion = factory.create(&lua, LuaMultiValue::new()).unwrap();
        assert_eq!(assertion.to_string(), "replaced");
    }
}
