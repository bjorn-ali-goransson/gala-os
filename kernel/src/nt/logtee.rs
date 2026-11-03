//! `logtee=C:/dir`: what programs write to files below that directory is
//! also logged, line by line (`logtee: <file>: <line>`), so a server's
//! own log file (Win32-OpenSSH's %ProgramData%\ssh\logs\sshd.log) ends up
//! in the serial log, and in a failed scenario's log, even when the VM is
//! stopped from outside (OBSERVABILITY: what a failure leaves behind).

use crate::sync::IrqMutex;
use alloc::vec::Vec;

/// The directory's volume path (`\ProgramData\ssh\logs`), lower case; empty:
/// off.
static PREFIX: IrqMutex<Vec<u16>> = IrqMutex::new(Vec::new());
static ACTIVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Sets the directory from the command line's (decoded) `C:/dir` value.
pub fn configure(dir: &str) {
    let rest = dir.strip_prefix("C:").or_else(|| dir.strip_prefix("c:")).unwrap_or(dir);
    let mut prefix = Vec::new();
    if prefix.try_reserve_exact(rest.len() + 1).is_err() {
        crate::error!("logtee: out of memory");
        return;
    }
    for c in rest.encode_utf16() {
        let c = if c == u16::from(b'/') { u16::from(b'\\') } else { c };
        let _ = crate::mm::heap::TryVec::try_push(&mut prefix, lower(c));
    }
    while prefix.last() == Some(&u16::from(b'\\')) {
        prefix.pop();
    }
    #[cfg(not(test))]
    crate::info!("logtee: files below {dir} are logged");
    *PREFIX.lock() = prefix;
    ACTIVE.store(true, core::sync::atomic::Ordering::Relaxed);
}

fn lower(c: u16) -> u16 {
    if (u16::from(b'A')..=u16::from(b'Z')).contains(&c) { c + 32 } else { c }
}

pub fn active() -> bool {
    ACTIVE.load(core::sync::atomic::Ordering::Relaxed)
}

/// The file name after the prefix, if `path` (a volume path) is below it.
pub fn below(path: &[u16]) -> Option<&[u16]> {
    let prefix = PREFIX.lock();
    let n = prefix.len();
    let inside = path.len() > n + 1
        && path[n] == u16::from(b'\\')
        && path[..n].iter().zip(prefix.iter()).all(|(&a, &b)| lower(a) == b);
    inside.then(|| &path[n + 1..])
}

/// Logs what was written to `name`: each line on its own.
pub fn log(name: &[u16], bytes: &[u8]) {
    let mut file = [0u8; 64];
    let mut n = 0;
    for c in char::decode_utf16(name.iter().copied()) {
        let mut b = [0u8; 4];
        let s = c.unwrap_or('?').encode_utf8(&mut b);
        if n + s.len() > file.len() {
            break;
        }
        file[n..n + s.len()].copy_from_slice(s.as_bytes());
        n += s.len();
    }
    let file = core::str::from_utf8(&file[..n]).unwrap_or("?");
    for line in bytes.split(|&b| b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if !line.is_empty() {
            crate::info!("logtee: {file}: {}", Text(line));
        }
    }
}

/// A log line as text: UTF-8 kept (U+FFFD for bad bytes), control
/// characters as `\xNN` so a line stays one line.
struct Text<'a>(&'a [u8]);

impl core::fmt::Display for Text<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for chunk in self.0.utf8_chunks() {
            for c in chunk.valid().chars() {
                if (c as u32) < 0x20 || c == '\x7f' {
                    write!(f, "\\x{:02x}", c as u32)?;
                } else {
                    core::fmt::Write::write_char(f, c)?;
                }
            }
            if !chunk.invalid().is_empty() {
                f.write_str("\u{fffd}")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_below_the_directory_are_found_in_any_case() {
        configure("C:/ProgramData/ssh/logs/");
        let path: Vec<u16> = "\\programdata\\SSH\\logs\\sshd.log".encode_utf16().collect();
        let name: Vec<u16> = "sshd.log".encode_utf16().collect();
        assert_eq!(below(&path), Some(&name[..]));
        let other: Vec<u16> = "\\ProgramData\\ssh\\logsx\\a".encode_utf16().collect();
        assert_eq!(below(&other), None);
        let dir: Vec<u16> = "\\ProgramData\\ssh\\logs".encode_utf16().collect();
        assert_eq!(below(&dir), None);
    }
}
