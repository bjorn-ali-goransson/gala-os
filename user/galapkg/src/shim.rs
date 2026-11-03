//! Command shims. winget links a portable package's commands into its
//! Links directory as symbolic links; gala has no file symbolic links
//! yet (CreateSymbolicLinkW: ERROR_PRIVILEGE_NOT_HELD), so a command is a
//! copy of `gala.exe` named `<alias>.exe` with a `<alias>.shim` text file
//! beside it:
//!
//! ```text
//! target = C:\Users\gala\AppData\Local\Microsoft\WinGet\Packages\<key>\dir\tool.exe
//! ```
//!
//! `gala.exe` started under any name other than `gala` looks for its
//! `.shim` and, when there is one, runs the target with the rest of its
//! own command line, passes its standard handles, waits and exits with
//! the target's exit code (user/gala, `shim`). The same idea as Scoop's
//! shims; the format is gala's own.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The `.shim` file for `target`.
pub fn format(target: &str) -> String {
    format!("target = {target}\r\n")
}

/// The target a `.shim` file names (None when it names none).
pub fn parse(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (k, v) = line.split_once('=')?;
        (k.trim().eq_ignore_ascii_case("target") && !v.trim().is_empty())
            .then(|| v.trim().to_string())
    })
}

fn is_space(c: u16) -> bool {
    c == u16::from(b' ') || c == u16::from(b'\t')
}

/// A command line without its first argument (the program name, quoted
/// or up to the first space, as CommandLineToArgvW reads it) and the
/// spaces after it: what the shim passes on unchanged.
pub fn arguments(command_line: &[u16]) -> &[u16] {
    let quote = u16::from(b'"');
    let end = if command_line.first() == Some(&quote) {
        command_line[1..].iter().position(|&c| c == quote).map_or(command_line.len(), |i| i + 2)
    } else {
        command_line.iter().position(|&c| is_space(c)).unwrap_or(command_line.len())
    };
    let rest = &command_line[end.min(command_line.len())..];
    let start = rest.iter().position(|&c| !is_space(c)).unwrap_or(rest.len());
    &rest[start..]
}

/// The target's command line: its path quoted, then `arguments`.
pub fn command_line(target: &str, arguments: &[u16]) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    out.push(u16::from(b'"'));
    out.extend(target.encode_utf16());
    out.push(u16::from(b'"'));
    if !arguments.is_empty() {
        out.push(u16::from(b' '));
        out.extend_from_slice(arguments);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn shim_file() {
        let text = format("C:\\P\\x y\\tool.exe");
        assert_eq!(parse(&text).as_deref(), Some("C:\\P\\x y\\tool.exe"));
        assert_eq!(parse("# nothing\r\n"), None);
        assert_eq!(parse("Target=C:\\a.exe"), Some("C:\\a.exe".into()));
    }

    #[test]
    fn arguments_after_the_program() {
        assert_eq!(arguments(&w("rg.exe -i \"a b\" c")), &w("-i \"a b\" c")[..]);
        assert_eq!(arguments(&w("\"C:\\L i\\rg.exe\"   --version")), &w("--version")[..]);
        assert_eq!(arguments(&w("rg")), &w("")[..]);
        assert_eq!(arguments(&w("\"unterminated")), &w("")[..]);
        assert_eq!(
            command_line("C:\\P\\rg.exe", &w("-i \"a b\"")),
            w("\"C:\\P\\rg.exe\" -i \"a b\"")
        );
        assert_eq!(command_line("C:\\P\\rg.exe", &[]), w("\"C:\\P\\rg.exe\""));
    }
}
