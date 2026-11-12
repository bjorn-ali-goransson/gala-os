//! gala's `wsl.exe` (ROADMAP CT step 2; ARCHITECTURE "The WSL facade"):
//! the parts without system calls, so they run under `cargo test` on the
//! host.
//!
//! - [`tar`]: a distribution's rootfs tarball, and [`gunzip`] for a
//!   `.tar.gz` one;
//! - [`args`]: the command line as wsl.exe reads it (its own first-word
//!   parsing, and CommandLineToArgvW's rules for `--exec` and the
//!   subcommands' arguments);
//! - [`list_verbose`]: `wsl -l -v`'s table;
//! - [`passwd_entry`]: a user's home and shell from the distribution's
//!   `/etc/passwd`;
//! - [`guid_string`]: a distribution's registry key name.
//!
//! Spec: Microsoft's WSL documentation and its published source
//! (github.com/microsoft/WSL, MIT; CLEAN_ROOM "WSL", THIRD_PARTY.md):
//! the messages, the `-l -v` format, the registry values. `no_std` with
//! `alloc`: `wsl.exe` is freestanding (`gala_heap`).

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod args;
pub mod tar;

use alloc::string::String;
use alloc::vec::Vec;

/// The bytes of a gzip member (RFC 1952) inflated; None if it is not
/// gzip or does not inflate. A plain archive is used as it is.
pub fn gunzip(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 18 || data[0] != 0x1f || data[1] != 0x8b || data[2] != 8 {
        return None;
    }
    let flags = data[3];
    let mut at = 10;
    if flags & 4 != 0 {
        let xlen = usize::from(u16::from_le_bytes([*data.get(at)?, *data.get(at + 1)?]));
        at += 2 + xlen;
    }
    for bit in [8u8, 16] {
        if flags & bit != 0 {
            at += data.get(at..)?.iter().position(|&c| c == 0)? + 1;
        }
    }
    if flags & 2 != 0 {
        at += 2;
    }
    miniz_oxide::inflate::decompress_to_vec(data.get(at..)?).ok()
}

/// Whether `data` starts like a gzip member.
pub fn is_gzip(data: &[u8]) -> bool {
    data.starts_with(&[0x1f, 0x8b])
}

/// One row of `wsl -l -v`.
pub struct Row<'a> {
    pub name: &'a str,
    pub state: &'a str,
    pub version: u32,
    pub default: bool,
}

/// `wsl -l -v`'s table as WSL prints it: `"%s %-<w>s%-16s%s\n"` with the
/// default's `*` (else a space) first, the name column as wide as the
/// longest name (at least "NAME") plus 4, the state 16 wide; a header
/// line first.
pub fn list_verbose(rows: &[Row<'_>]) -> String {
    let width = rows.iter().map(|r| r.name.chars().count()).max().unwrap_or(0).max(4) + 4;
    let mut out = String::new();
    let mut line = |mark: &str, name: &str, state: &str, version: &str| {
        out.push_str(mark);
        out.push(' ');
        out.push_str(name);
        for _ in name.chars().count()..width {
            out.push(' ');
        }
        out.push_str(state);
        for _ in state.chars().count()..16 {
            out.push(' ');
        }
        out.push_str(version);
        out.push('\n');
    };
    line(" ", "NAME", "STATE", "VERSION");
    for r in rows {
        let mut v = [0u8; 10];
        line(if r.default { "*" } else { " " }, r.name, r.state, decimal(r.version, &mut v));
    }
    out
}

/// `v` in decimal, in `buf`.
pub fn decimal(mut v: u32, buf: &mut [u8; 10]) -> &str {
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    core::str::from_utf8(&buf[i..]).unwrap_or("0")
}

/// A user's entry in a passwd file: (name, home, shell).
pub fn passwd_entry(passwd: &[u8], uid: u32) -> Option<(&[u8], &[u8], &[u8])> {
    for line in passwd.split(|&c| c == b'\n') {
        let f: Vec<&[u8]> = line.split(|&c| c == b':').collect();
        if f.len() < 7 {
            continue;
        }
        let id = core::str::from_utf8(f[2]).ok().and_then(|s| s.trim().parse::<u32>().ok());
        if id == Some(uid) {
            let shell = f[6].strip_suffix(b"\r").unwrap_or(f[6]);
            return Some((f[0], f[5], shell));
        }
    }
    None
}

/// A GUID from 16 random bytes (version 4, RFC 4122 variant) as WSL
/// names a distribution's key: `{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}`,
/// lowercase (its GUID_FORMAT_STRING).
pub fn guid_string(mut b: [u8; 16]) -> String {
    b[7] = (b[7] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let d1 = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let d2 = u16::from_le_bytes([b[4], b[5]]);
    let d3 = u16::from_le_bytes([b[6], b[7]]);
    let mut s = String::new();
    let hex = |s: &mut String, v: u64, digits: u32| {
        for i in (0..digits).rev() {
            let d = ((v >> (4 * i)) & 0xf) as u8;
            s.push(char::from(if d < 10 { b'0' + d } else { b'a' + d - 10 }));
        }
    };
    s.push('{');
    hex(&mut s, u64::from(d1), 8);
    s.push('-');
    hex(&mut s, u64::from(d2), 4);
    s.push('-');
    hex(&mut s, u64::from(d3), 4);
    s.push('-');
    hex(&mut s, u64::from(b[8]), 2);
    hex(&mut s, u64::from(b[9]), 2);
    s.push('-');
    for &x in &b[10..] {
        hex(&mut s, u64::from(x), 2);
    }
    s.push('}');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verbose_list_is_wsls_format() {
        let rows = [
            Row { name: "Ubuntu", state: "Running", version: 2, default: true },
            Row { name: "bb", state: "Stopped", version: 2, default: false },
        ];
        assert_eq!(
            list_verbose(&rows),
            "  NAME      STATE           VERSION\n\
             * Ubuntu    Running         2\n  \
             bb        Stopped         2\n"
        );
        // Names shorter than "NAME": that column is 8 wide.
        let rows = [Row { name: "a", state: "Stopped", version: 2, default: true }];
        assert_eq!(
            list_verbose(&rows),
            "  NAME    STATE           VERSION\n* a       Stopped         2\n"
        );
    }

    #[test]
    fn passwd_gives_home_and_shell() {
        let p = b"root:x:0:0:root:/root:/bin/sh\nu:x:1000:1000::/home/u:/bin/ash\r\n";
        assert_eq!(passwd_entry(p, 0), Some((&b"root"[..], &b"/root"[..], &b"/bin/sh"[..])));
        assert_eq!(passwd_entry(p, 1000).unwrap().2, b"/bin/ash");
        assert_eq!(passwd_entry(p, 5), None);
    }

    #[test]
    fn guids_are_braced_lowercase_version_4() {
        let g = guid_string([0xab; 16]);
        assert_eq!(g, "{abababab-abab-4bab-abab-abababababab}");
        assert_eq!(g.len(), 38);
    }

    #[test]
    fn gzip_members_inflate() {
        // `printf hi | gzip -n`.
        let gz = [
            0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3, 0xcb, 0xc8, 4, 0, 0xac, 0x2a, 0x93, 0xd8, 2, 0, 0,
            0,
        ];
        assert_eq!(gunzip(&gz).as_deref(), Some(&b"hi"[..]));
        assert!(is_gzip(&gz));
        assert_eq!(gunzip(b"plain text, not gzip"), None);
    }
}
