use std::collections::HashMap;

use miette::Result;
use mlua::prelude::*;

use crate::assertions::Assertion;
use crate::assertions::Brew;
use crate::assertions::BrewBundle;
use crate::assertions::Debug;
use crate::assertions::Directory;
use crate::assertions::Git;
use crate::assertions::LuaAssertion;
use crate::assertions::LuaProvider;
use crate::assertions::ProviderFactory;
use crate::assertions::Symlink;
use crate::assertions::TypedFactory;
use crate::error::FrorkError;

/// Dispatches a provider name to the factory that builds its assertions.
/// Providers defined in Fennel shadow the built-ins.
#[derive(Default)]
pub struct Providers {
    lua_providers: HashMap<String, LuaProvider>,
}

impl Providers {
    pub fn provide(&mut self, name: &str, lua_provider: LuaProvider) {
        self.lua_providers.insert(name.to_string(), lua_provider);
    }

    pub fn get_factory(&self, name: &str) -> Result<Box<dyn ProviderFactory>> {
        if let Some(lua_provider) = self.lua_providers.get(name) {
            return Ok(Box::new(LuaAssertionFactory {
                name: name.to_string(),
                lua_provider: lua_provider.clone(),
            }));
        }

        match name {
            "brew" => Ok(Box::new(TypedFactory::<Brew>::new())),
            "brew-bundle" => Ok(Box::new(TypedFactory::<BrewBundle>::new())),
            "debug" => Ok(Box::new(TypedFactory::<Debug>::new())),
            "directory" => Ok(Box::new(TypedFactory::<Directory>::new())),
            "git" => Ok(Box::new(TypedFactory::<Git>::new())),
            "symlink" => Ok(Box::new(TypedFactory::<Symlink>::new())),
            _ => Err(FrorkError::UnknownProvider {
                name: name.to_string(),
            }
            .into()),
        }
    }
}

struct LuaAssertionFactory {
    name: String,
    lua_provider: LuaProvider,
}

impl ProviderFactory for LuaAssertionFactory {
    fn build(&self, _lua: &Lua, args: LuaMultiValue) -> Result<Box<dyn Assertion>> {
        Ok(Box::new(LuaAssertion::new(
            &self.name,
            args,
            self.lua_provider.clone(),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua_provider(lua: &Lua) -> LuaProvider {
        let value = lua
            .load(r#"return {status = function() return "present" end, create = function() end}"#)
            .eval::<LuaValue>()
            .unwrap();
        LuaProvider::from_lua(value, lua).unwrap()
    }

    /// Every built-in name resolves to a factory that builds a working
    /// assertion, checked through the factory rather than by name alone.
    #[test]
    fn test_built_in_types_resolve() {
        let lua = Lua::new();
        let providers = Providers::default();

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
            let factory = providers.get_factory(name);
            let factory = factory.unwrap_or_else(|e| panic!("{name} did not resolve: {e}"));
            let assertion = factory.build(&lua, args);
            let assertion = assertion.unwrap_or_else(|e| panic!("{name} failed to build: {e}"));
            assert_eq!(assertion.to_string(), expected, "for type {name}");
        }
    }

    #[test]
    fn test_provided_types_are_dispatched() {
        let lua = Lua::new();
        let mut providers = Providers::default();
        providers.provide("custom", lua_provider(&lua));

        let factory = providers.get_factory("custom").unwrap();
        let assertion = factory
            .build(&lua, lua.load(r#"return "arg""#).eval().unwrap())
            .unwrap();

        assert_eq!(assertion.to_string(), "custom arg");
    }

    #[test]
    fn test_re_providing_replaces_the_previous_type() {
        let lua = Lua::new();
        let mut providers = Providers::default();

        providers.provide("custom", lua_provider(&lua));
        let replacement = lua
            .load(
                r#"return {
                    display = function() return "replaced" end,
                    status = function() return "present" end,
                    create = function() end,
                }"#,
            )
            .eval::<LuaValue>()
            .unwrap();
        providers.provide("custom", LuaProvider::from_lua(replacement, &lua).unwrap());

        let factory = providers.get_factory("custom").unwrap();
        let assertion = factory.build(&lua, LuaMultiValue::new()).unwrap();
        assert_eq!(assertion.to_string(), "replaced");
    }
}
