//! `frork compile`: a Fennel script to a standalone executable.
//!
//! The executable is a frork binary for the target with the script, compiled
//! to Lua, appended after it. At startup frork looks for that payload at the
//! end of its own file and runs it instead of the CLI. Compiling is therefore
//! only Fennel and copying bytes: no C compiler, and any host can build for
//! any target it has a frork binary for.

use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use mlua::prelude::*;
use xdg::BaseDirectories;

use crate::error::CompileError;

type Result<T> = std::result::Result<T, CompileError>;

/// Where releases publish frork binaries, one `frork-<target>.tar.gz` per tag.
pub const RELEASES_URL: &str = "https://github.com/kejadlen/frork/releases/download";

/// Ends every payload, after its length as a little-endian `u64`.
const MAGIC: &[u8; 8] = b"frork\0lu";
const TRAILER_LEN: u64 = 16;

/// Compiles `script` to Lua, inlining the modules it requires from its own
/// directory (macros included) so the result stands alone.
pub fn compile_lua(script: &str) -> Result<String> {
    let dir = Path::new(script).parent().and_then(Path::to_str);
    let dir = dir.filter(|d| !d.is_empty()).unwrap_or(".");
    let source = fs_err::read_to_string(script)?;

    let lua_err = |e: LuaError| CompileError::Fennel {
        script: script.to_string(),
        message: e.to_string(),
    };
    let lua = Lua::new();
    let fennel: LuaTable = lua
        .load(include_str!("../fennel-1.6.0.lua"))
        .set_name("=fennel")
        .eval()
        .map_err(lua_err)?;
    lua.register_module("fennel", &fennel).map_err(lua_err)?;
    for key in ["path", "macro-path"] {
        let default: String = fennel.get(key).map_err(lua_err)?;
        let path = format!("{dir}/?.fnl;{dir}/?/init.fnl;{dir}/?/init-macros.fnl;{default}");
        fennel.set(key, path).map_err(lua_err)?;
    }

    let options = lua.create_table().map_err(lua_err)?;
    options.set("filename", script).map_err(lua_err)?;
    options.set("requireAsInclude", true).map_err(lua_err)?;
    // The engine comes from the binary the script runs in.
    let skip = lua.create_sequence_from(["frork"]).map_err(lua_err)?;
    options.set("skipInclude", skip).map_err(lua_err)?;
    let compile: LuaFunction = fennel.get("compileString").map_err(lua_err)?;
    compile.call((source, options)).map_err(lua_err)
}

/// `runner`, without any payload it already carries, followed by `lua`.
pub fn bundle(runner: &[u8], lua: &str) -> Vec<u8> {
    let runner = strip(runner);
    let len = u64::try_from(lua.len()).unwrap_or(u64::MAX);
    [runner, lua.as_bytes(), &len.to_le_bytes(), MAGIC].concat()
}

/// `exe` without its payload, if it has one.
fn strip(exe: &[u8]) -> &[u8] {
    let Some((body, trailer)) = exe.split_last_chunk::<16>() else {
        return exe;
    };
    let Some(len) = payload_len(trailer) else {
        return exe;
    };
    let len = usize::try_from(len).unwrap_or(usize::MAX);
    body.len()
        .checked_sub(len)
        .and_then(|end| body.get(..end))
        .unwrap_or(exe)
}

/// The payload's length, if `trailer` ends one.
fn payload_len(trailer: &[u8; 16]) -> Option<u64> {
    let (len, magic) = trailer.split_first_chunk::<8>()?;
    (magic == MAGIC).then(|| u64::from_le_bytes(*len))
}

/// The Lua payload at the end of `exe`, if there is one. Only the trailer
/// and the payload are read, since this runs on every frork start.
pub fn payload(mut exe: impl Read + Seek) -> std::io::Result<Option<Vec<u8>>> {
    let size = exe.seek(SeekFrom::End(0))?;
    let Some(trailer_at) = size.checked_sub(TRAILER_LEN) else {
        return Ok(None);
    };
    let mut trailer = [0; 16];
    exe.seek(SeekFrom::Start(trailer_at))?;
    exe.read_exact(&mut trailer)?;
    let Some(len) = payload_len(&trailer) else {
        return Ok(None);
    };
    let Some(start) = trailer_at.checked_sub(len) else {
        return Ok(None);
    };
    exe.seek(SeekFrom::Start(start))?;
    let mut payload = Vec::new();
    exe.take(len).read_to_end(&mut payload)?;
    Ok(Some(payload))
}

/// The frork binaries `frork compile` builds on for other targets, cached
/// under frork's XDG cache directory as releases publish them.
pub struct Runners {
    dirs: BaseDirectories,
    releases_url: String,
}

impl Default for Runners {
    fn default() -> Self {
        Self::new(BaseDirectories::with_prefix("frork"), RELEASES_URL)
    }
}

impl Runners {
    pub fn new(dirs: BaseDirectories, releases_url: &str) -> Self {
        Self {
            dirs,
            releases_url: releases_url.to_string(),
        }
    }

    /// The frork binary for `target` matching this frork's `version`,
    /// downloaded the first time it is needed.
    pub fn get(&self, version: &str, target: &str) -> Result<PathBuf> {
        if version.ends_with("-dev") {
            return Err(CompileError::DevelopmentBuild {
                version: version.to_string(),
            });
        }
        let path = Path::new("runners")
            .join(version)
            .join(target)
            .join("frork");
        if let Some(runner) = self.dirs.find_cache_file(&path) {
            return Ok(runner);
        }
        let runner = self.dirs.place_cache_file(&path)?;
        let tag = format!("v{version}").replace('+', "%2B");
        let url = format!("{}/{tag}/frork-{target}.tar.gz", self.releases_url);
        fetch(&url, &runner)?;
        Ok(runner)
    }
}

/// Downloads a release archive and moves the `frork` in it to `dest`.
fn fetch(url: &str, dest: &Path) -> Result<()> {
    let partial = dest.with_extension("partial");
    let archive = partial.join("frork.tar.gz");
    fs_err::create_dir_all(&partial)?;
    run(Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&archive)
        .arg(url))?;
    run(Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&partial))?;
    fs_err::rename(partial.join("frork"), dest)?;
    Ok(fs_err::remove_dir_all(&partial)?)
}

fn run(command: &mut Command) -> Result<()> {
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(CompileError::Command {
            command: format!("{command:?}"),
            status,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tempfile::TempDir;

    #[test]
    fn test_compile_lua_inlines_modules_and_macros() {
        let dir = TempDir::new().unwrap();
        fs_err::write(dir.path().join("helper.fnl"), "{:x \"from-helper\"}").unwrap();
        fs_err::write(
            dir.path().join("macros.fnl"),
            "{:twice (fn [x] `(.. ,x ,x))}",
        )
        .unwrap();
        let script = dir.path().join("main.fnl");
        fs_err::write(
            &script,
            "(import-macros {: twice} :macros)
             (local {: x} (require :helper))
             (local frork (require :frork))
             (twice x)",
        )
        .unwrap();

        let lua = compile_lua(script.to_str().unwrap()).unwrap();

        assert!(lua.contains("from-helper"));
        assert!(lua.contains(r#"require("frork")"#));
        assert!(!lua.contains("twice"));
    }

    #[test]
    fn test_compile_lua_reports_a_missing_script() {
        let error = compile_lua("frork-missing-script.fnl").unwrap_err();
        assert!(error.to_string().contains("frork-missing-script.fnl"));
    }

    #[test]
    fn test_compile_lua_reports_fennel_errors() {
        let dir = TempDir::new().unwrap();
        let script = dir.path().join("main.fnl");
        fs_err::write(&script, "(local x").unwrap();

        let error = compile_lua(script.to_str().unwrap()).unwrap_err();
        assert!(matches!(error, CompileError::Fennel { .. }));
    }

    #[test]
    fn test_payload_round_trips_through_bundle() {
        let exe = bundle(b"runner bytes", "print('hi')");

        assert!(exe.starts_with(b"runner bytes"));
        let payload = payload(Cursor::new(&exe)).unwrap();
        assert_eq!(payload.as_deref(), Some(&b"print('hi')"[..]));
    }

    #[test]
    fn test_bundle_replaces_an_existing_payload() {
        let once = bundle(b"runner", "first");
        let twice = bundle(&once, "second");

        assert_eq!(twice, bundle(b"runner", "second"));
    }

    #[test]
    fn test_payload_is_none_for_a_plain_binary() {
        assert_eq!(payload(Cursor::new(b"short")).unwrap(), None);
        assert_eq!(payload(Cursor::new([7; 64])).unwrap(), None);
        assert_eq!(strip(b"short"), b"short");
        assert_eq!(strip(&[7; 64]), &[7; 64]);
    }

    #[test]
    fn test_payload_ignores_a_length_longer_than_the_file() {
        let mut exe = 100u64.to_le_bytes().to_vec();
        exe.extend_from_slice(MAGIC);

        assert_eq!(payload(Cursor::new(&exe)).unwrap(), None);
        assert_eq!(strip(&exe), &exe[..]);
    }

    fn release(dir: &Path, version: &str, target: &str) -> PathBuf {
        // curl decodes the %2B in the URL, so the file sits under a +.
        let tag = dir.join("releases").join(format!("v{version}"));
        let staged = dir.join("staged");
        fs_err::create_dir_all(&tag).unwrap();
        fs_err::create_dir_all(&staged).unwrap();
        fs_err::write(staged.join("frork"), "runner").unwrap();
        let archive = tag.join(format!("frork-{target}.tar.gz"));
        run(Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&staged)
            .arg("frork"))
        .unwrap();
        archive
    }

    /// Runners cached under `dir`, downloaded from releases under it too.
    fn runners(dir: &Path) -> Runners {
        let mut dirs = BaseDirectories::with_prefix("frork");
        dirs.cache_home = Some(dir.join("cache"));
        let url = format!("file://{}", dir.join("releases").display());
        Runners::new(dirs, &url)
    }

    #[test]
    fn test_runners_download_once_then_reuse() {
        let dir = TempDir::new().unwrap();
        let version = "2026-10-20+abc1234";
        let target = "x86_64-unknown-linux-gnu";
        let archive = release(dir.path(), version, target);
        let runners = runners(dir.path());

        let runner = runners.get(version, target).unwrap();
        assert!(runner.starts_with(dir.path().join("cache/frork/runners")));
        assert_eq!(fs_err::read_to_string(&runner).unwrap(), "runner");
        assert!(!runner.with_extension("partial").exists());

        // A second lookup must not download: the archive is gone.
        fs_err::remove_file(&archive).unwrap();
        let again = runners.get(version, target).unwrap();
        assert_eq!(again, runner);
        assert_eq!(fs_err::read_to_string(&again).unwrap(), "runner");
    }

    #[test]
    fn test_runners_report_a_missing_release() {
        let dir = TempDir::new().unwrap();

        let error = runners(dir.path())
            .get("2026-01-01+0000000", "x86_64-apple-darwin")
            .unwrap_err();
        assert!(matches!(error, CompileError::Command { .. }));
    }

    #[test]
    fn test_runners_need_a_cache_directory() {
        let mut dirs = BaseDirectories::with_prefix("frork");
        dirs.cache_home = None;

        let error = Runners::new(dirs, RELEASES_URL)
            .get("2026-01-01+0000000", "x86_64-apple-darwin")
            .unwrap_err();
        assert!(matches!(error, CompileError::Io(_)));
    }

    #[test]
    fn test_runners_refuse_development_builds() {
        let error = Runners::default()
            .get("2026-10-20+abc-dev", "x")
            .unwrap_err();
        assert!(error.to_string().contains("--runner"));
    }

    #[test]
    fn test_run_reports_a_missing_program() {
        let error = run(&mut Command::new("/frork-does-not-exist")).unwrap_err();
        assert!(matches!(error, CompileError::Io(_)));
    }
}
