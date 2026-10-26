//! `frork compile`: a Fennel script to a standalone executable, through
//! Fennel's own `--compile-binary`.
//!
//! The executable links three things built for its target: Lua, the frork
//! engine as a native Lua module, and the script. The first two come from an
//! SDK for the target (see `bin/build-sdk`), so compiling needs only a C
//! compiler (zig, for cross-compiling) and no Rust toolchain.

use std::path::Path;
use std::path::PathBuf;

use miette::Result;
use miette::miette;
use mlua::prelude::*;

/// Runs Fennel's binary compiler with the SDK's paths and compiler settings.
/// Fennel reads those from the environment and ends with `os.exit`, so both
/// are stood in for while it runs rather than touching the real process.
const COMPILE_LUA: &str = r#"
local fennel, binary, p = ...
for _, key in ipairs({"path", "macro-path"}) do
  fennel[key] = table.concat({p.dir .. "/?.fnl", p.dir .. "/?/init.fnl",
                              p.dir .. "/?/init-macros.fnl", fennel[key]}, ";")
end
local getenv, exit = os.getenv, os.exit
os.getenv = function(name)
  local value = p.env[name]
  if value == nil then return getenv(name) end
  return value
end
local exited = {}
os.exit = function(code) exited.code = code; error(exited) end
local options = {filename = p.script, requireAsInclude = true, skipInclude = {"frork"}}
local ok, err = pcall(binary.compile, p.script, p.output, p.liblua, p.include,
                      options, {"--native-module", p.libfrork})
os.getenv, os.exit = getenv, exit
if ok or err == exited then return exited.code end
error(err, 0)
"#;

/// The per-target libraries and headers a compiled script links against.
#[derive(Debug)]
pub struct Sdk {
    dir: PathBuf,
}

impl Sdk {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, name: &str) -> Result<String> {
        let path = self.dir.join(name);
        path.to_str()
            .map(str::to_string)
            .ok_or_else(|| miette!("SDK path is not UTF-8: {}", path.display()))
    }

    /// The system libraries Rust's standard library needs on the target.
    fn link_flags(&self) -> Result<String> {
        let flags = fs_err::read_to_string(self.dir.join("link-flags"));
        Ok(flags.map_err(|e| miette!("{e}"))?.trim().to_string())
    }
}

/// The C compiler for `target`: zig, which cross-compiles to every target
/// `bin/build-sdk` builds an SDK for.
pub fn zig_cc(target: &str) -> Result<String> {
    let zig_target = match target {
        "aarch64-apple-darwin" => "aarch64-macos",
        "x86_64-apple-darwin" => "x86_64-macos",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        _ => return Err(miette!("No known zig target for {target}")),
    };
    Ok(format!("zig cc -target {zig_target}"))
}

/// Compiles `script` into the executable `output` for the SDK's target.
pub fn compile_binary(script: &str, output: &str, sdk: &Sdk, cc: &str) -> Result<()> {
    let lua = Lua::new();
    let fennel: LuaTable = lua
        .load(include_str!("../fennel-1.6.0.lua"))
        .set_name("=fennel")
        .eval()
        .map_err(|e| miette!("Failed to load Fennel: {e}"))?;
    lua.register_module("fennel", &fennel)
        .map_err(|e| miette!("Failed to register fennel module: {e}"))?;
    let binary: LuaTable = lua
        .load(include_str!("../fennel-binary-1.6.0.lua"))
        .set_name("=fennel.binary")
        .eval()
        .map_err(|e| miette!("Failed to load Fennel's binary compiler: {e}"))?;

    let dir = Path::new(script).parent().and_then(Path::to_str);
    let dir = dir.filter(|d| !d.is_empty()).unwrap_or(".");
    let env = [
        ("CC", cc.to_string()),
        ("CC_OPTS", sdk.link_flags()?),
        // Fennel lists the archive's luaopen_* symbols with nm to register
        // them, and the host's nm may not read the target's object format.
        // The SDK's archive has exactly one, so say so instead.
        ("NM", "printf ' T _luaopen_frork\\n' #".to_string()),
    ];
    let params = [
        ("script", script.to_string()),
        ("dir", dir.to_string()),
        ("output", output.to_string()),
        ("liblua", sdk.path("liblua.a")?),
        ("include", sdk.path("include")?),
        ("libfrork", sdk.path("libfrork.a")?),
    ];

    let lua_err = |e: LuaError| miette!("Failed to compile binary: {e}");
    let params_table = lua.create_table_from(params).map_err(lua_err)?;
    let env_table = lua.create_table_from(env).map_err(lua_err)?;
    params_table.set("env", env_table).map_err(lua_err)?;

    let compile = lua.load(COMPILE_LUA).set_name("=frork.compile");
    let code: i64 = compile
        .call((fennel, binary, params_table))
        .map_err(lua_err)?;
    if code == 0 {
        Ok(())
    } else {
        Err(miette!(
            "The C compiler failed; the command is printed above"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// An SDK whose files exist but are never read: the stand-in compilers
    /// in these tests ignore their arguments.
    fn fake_sdk(dir: &Path) -> Sdk {
        fs_err::create_dir_all(dir.join("include")).unwrap();
        fs_err::write(dir.join("link-flags"), "-lm\n").unwrap();
        Sdk::new(dir)
    }

    fn script(dir: &Path) -> String {
        fs_err::write(dir.join("helper.fnl"), "{:x 1}").unwrap();
        let script = dir.join("main.fnl");
        fs_err::write(&script, "(local {: x} (require :helper)) (print x)").unwrap();
        script.to_str().unwrap().to_string()
    }

    #[test]
    fn test_compile_binary_runs_the_c_compiler_on_fennels_output() {
        let dir = TempDir::new().unwrap();
        let sdk = fake_sdk(&dir.path().join("sdk"));
        let script = script(dir.path());
        let output = dir.path().join("out");
        // The stand-in compiler keeps the C file Fennel hands it, which
        // embeds the script and its inlined helper module.
        let cc = dir.path().join("cc");
        fs_err::write(
            &cc,
            format!(
                "#!/bin/sh\nfor a; do case $a in *.c) cp \"$a\" {};; esac; done\n",
                output.display()
            ),
        )
        .unwrap();
        fs_err::set_permissions(&cc, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let cc = cc.to_str().unwrap();

        compile_binary(&script, output.to_str().unwrap(), &sdk, cc).unwrap();

        let c = fs_err::read_to_string(&output).unwrap();
        assert!(c.contains("luaopen_frork"));
        // The helper is inlined as Lua, which the C file holds as hex bytes.
        let hex = |s: &str| {
            s.bytes()
                .map(|b| format!("0x{b:02x}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        assert!(c.contains(&hex("helper")));
        // Fennel cleans up the C file it wrote next to the script.
        assert!(!dir.path().join("main.fnl_binary.c").exists());
    }

    #[test]
    fn test_compile_binary_reports_a_failing_c_compiler() {
        let dir = TempDir::new().unwrap();
        let sdk = fake_sdk(&dir.path().join("sdk"));
        let script = script(dir.path());

        let error = compile_binary(&script, "out", &sdk, "false").unwrap_err();
        assert!(error.to_string().contains("C compiler failed"));
    }

    #[test]
    fn test_compile_binary_reports_a_missing_script() {
        let dir = TempDir::new().unwrap();
        let sdk = fake_sdk(dir.path());

        // Fennel opens the C file before reading the script, so keep it in
        // the temporary directory.
        let missing = dir.path().join("nope.fnl");
        let error = compile_binary(missing.to_str().unwrap(), "out", &sdk, "true").unwrap_err();
        assert!(error.to_string().contains("Failed to compile binary"));
    }

    #[test]
    fn test_compile_binary_needs_the_sdks_link_flags() {
        let dir = TempDir::new().unwrap();

        let sdk = Sdk::new(dir.path());
        assert!(compile_binary("main.fnl", "out", &sdk, "true").is_err());
    }

    #[test]
    fn test_zig_cc_maps_rust_targets() {
        assert_eq!(
            zig_cc("aarch64-apple-darwin").unwrap(),
            "zig cc -target aarch64-macos"
        );
        assert_eq!(
            zig_cc("x86_64-apple-darwin").unwrap(),
            "zig cc -target x86_64-macos"
        );
        assert_eq!(
            zig_cc("aarch64-unknown-linux-gnu").unwrap(),
            "zig cc -target aarch64-linux-gnu"
        );
        assert_eq!(
            zig_cc("x86_64-unknown-linux-gnu").unwrap(),
            "zig cc -target x86_64-linux-gnu"
        );
        assert!(zig_cc("wasm32-unknown-unknown").is_err());
    }

    #[test]
    fn test_sdk_paths_must_be_utf8() {
        use std::os::unix::ffi::OsStrExt as _;

        let sdk = Sdk::new(std::ffi::OsStr::from_bytes(b"/\xff"));
        assert!(sdk.path("liblua.a").is_err());
    }
}
