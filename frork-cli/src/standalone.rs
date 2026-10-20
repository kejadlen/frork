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

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal arm64 Mach-O executable, from libsui's own test fixtures.
    const MACHO: &[u8] = include_bytes!("../tests/fixtures/exec_mach64");

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn test_a_plain_executable_has_no_embedded_script() {
        assert_eq!(embedded_script().unwrap(), None);
    }

    #[test]
    fn test_embed_into_elf() {
        let exe = fs_err::read(std::env::current_exe().unwrap()).unwrap();

        let out = embed(exe, "print('elf')").unwrap();

        assert!(libsui::utils::is_elf(&out));
        assert!(contains(&out, b"print('elf')"));
    }

    #[test]
    fn test_embed_into_macho() {
        let out = embed(MACHO.to_vec(), "print('macho')").unwrap();

        assert!(libsui::utils::is_macho(&out));
        assert!(contains(&out, b"print('macho')"));
    }

    #[test]
    fn test_embed_rejects_a_truncated_macho() {
        let truncated = MACHO.get(..64).unwrap().to_vec();

        assert!(embed(truncated, "").is_err());
    }

    #[test]
    fn test_embed_rejects_other_formats() {
        assert!(embed(b"#!/bin/sh".to_vec(), "").is_err());
    }
}
