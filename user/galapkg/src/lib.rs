//! galapkg: installing software from winget manifests (ROADMAP PK,
//! ARCHITECTURE "Installing software"). The parts that need no system
//! calls live here, so they run under `cargo test` on the host:
//!
//! - [`yaml`]: the YAML subset winget manifests use;
//! - [`manifest`]: singleton and multi-file manifests (schema 1.x),
//!   installer selection;
//! - [`version`]: winget's version ordering;
//! - [`zip`]: zip archives (stored and deflate, CRC-32 checked, no path
//!   leaving the target directory);
//! - [`source`]: a winget-pkgs tree (a local snapshot) as packages;
//! - [`install`]: install, uninstall, upgrade and list as transactions
//!   over a [`install::Host`] (files, registry, downloads, the log), which
//!   `gala.exe` implements with Win32 calls and the tests in memory;
//! - [`shim`]: the command shims in the Links directory.
//!
//! `no_std` with `alloc`: `gala.exe` is freestanding (the process heap as
//! the global allocator, `gala_heap`).

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod install;
pub mod manifest;
pub mod shim;
pub mod source;
pub mod version;
pub mod yaml;
pub mod zip;

use alloc::string::String;

/// SHA-256 of `data` as upper-case hex (as manifests write
/// InstallerSha256).
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(data);
    let mut s = String::new();
    for b in digest {
        for nibble in [b >> 4, b & 0xf] {
            s.push(char::from(if nibble < 10 { b'0' + nibble } else { b'A' + nibble - 10 }));
        }
    }
    s
}

/// ASCII case-insensitive equality.
pub fn eq_ci(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// ASCII case-insensitive substring test.
pub fn contains_ci(haystack: &str, needle: &str) -> bool {
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    n.is_empty() || h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_values() {
        // FIPS 180-4 examples.
        assert_eq!(
            sha256_hex(b"abc"),
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        );
        assert_eq!(
            sha256_hex(b""),
            "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855"
        );
    }

    #[test]
    fn case_insensitive_helpers() {
        assert!(eq_ci("BurntSushi.ripgrep.MSVC", "burntsushi.RIPGREP.msvc"));
        assert!(contains_ci("RipGrep MSVC", "grep"));
        assert!(!contains_ci("jq", "grep"));
    }
}
