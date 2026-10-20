//! Standalone binaries: a compiled script embedded in a copy of a frork
//! executable, which runs it instead of the usual CLI.
//!
//! The runtime executable can be built for another platform than the one
//! doing the embedding, since injection only rewrites the object file.

use miette::Result;
use miette::miette;

/// The section holding the compiled script. Mach-O caps names at 16 bytes.
pub const SECTION: &str = "frork_script";

/// The compiled script embedded in the running executable, if any.
pub fn embedded_script() -> Result<Option<String>> {
    let Some(bytes) = libsui::find_section(SECTION).map_err(|e| miette!("{e}"))? else {
        return Ok(None);
    };
    let code = String::from_utf8(bytes.to_vec())
        .map_err(|e| miette!("Embedded script is not UTF-8: {e}"))?;
    Ok(Some(code))
}

/// Returns `runtime` (the bytes of an ELF or Mach-O frork executable) with
/// `lua_code` embedded. Mach-O output is ad-hoc signed, which Apple Silicon
/// requires before it will run.
pub fn embed(runtime: Vec<u8>, lua_code: &str) -> Result<Vec<u8>> {
    let data = lua_code.as_bytes().to_vec();
    let mut out = Vec::new();
    if libsui::utils::is_elf(&runtime) {
        libsui::Elf::new(&runtime)
            .append(SECTION, &data, &mut out)
            .map_err(|e| miette!("Failed to embed into ELF: {e}"))?;
    } else if libsui::utils::is_macho(&runtime) {
        libsui::Macho::from(runtime)
            .and_then(|m| m.write_section(SECTION, data))
            .and_then(|m| m.build_and_sign(&mut out))
            .map_err(|e| miette!("Failed to embed into Mach-O: {e}"))?;
    } else {
        return Err(miette!("Runtime is neither an ELF nor a Mach-O executable"));
    }
    Ok(out)
}
