use frork_lib::assertions::Assertion;
use frork_lib::assertions::Status;
use frork_lib::report;
use frork_lib::report::Action;
use frork_lib::runtime::Declaration;
use frork_lib::runtime::frork_module;
use mlua::prelude::*;

// The frork engine as a Lua module, `require("frork")`. Linked statically
// into a binary from `fennel --compile-binary --native-module`, it turns a
// Fennel script into a standalone executable: `--apply` creates absent `ok`
// assertions and removes present or drifted `no` ones, otherwise they are
// only reported. Drifted `ok` assertions are reported but never updated.

#[mlua::lua_module]
fn frork(lua: &Lua) -> LuaResult<LuaTable> {
    let apply = std::env::args().any(|arg| arg == "--apply");
    let handler = move |declaration: Declaration, status: &Status, assertion: &dyn Assertion| {
        print_lines(&report::render(declaration, status, assertion));
        if !apply {
            return Ok(());
        }
        match report::action(declaration, status) {
            Action::Create => {
                assertion.create()?;
                print_lines(&report::render(declaration, &Status::Present, assertion));
            }
            Action::Remove => {
                assertion.remove()?;
                print_lines(&report::render(declaration, &Status::Absent, assertion));
            }
            Action::Nothing | Action::PromptUpdate => {}
        }
        Ok(())
    };
    let exports = frork_module(lua, handler)?;
    exports.set("VERSION", frork_lib::VERSION)?;
    Ok(exports)
}

fn print_lines(lines: &[String]) {
    for line in lines {
        println!("{line}");
    }
}
