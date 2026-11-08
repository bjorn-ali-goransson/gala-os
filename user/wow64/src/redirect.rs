//! The file system redirector (Microsoft Learn, "File System
//! Redirector"): a 32-bit program's opens of `%windir%\System32` go to
//! `%windir%\SysWOW64`, except the documented exemptions (`catroot`,
//! `catroot2`, `driverstore`, `drivers\etc`, `logfiles`, `spool`);
//! `%windir%\Sysnative` names the real System32, and `%windir%\regedit.exe`
//! is SysWOW64's. Per thread: Wow64DisableWow64FsRedirection turns it off
//! (the 32-bit ntdll's RtlWow64EnableFsRedirectionEx, through
//! `GalaWow64FsRedirection`), with the flag where Windows keeps it,
//! TEB64.TlsSlots[8] (WOW64_TLS_FILESYSREDIR).
//!
//! In wow64.dll because the switch is per thread and the rewrite is string
//! work on the name the caller passed; the kernel's namespace stays
//! unaware. It applies to the names of the file services' OBJECT_ATTRIBUTES
//! (NtCreateFile, NtOpenFile, NtQueryAttributesFile) given as full NT paths
//! (`\??\C:\Windows\System32\...`, no RootDirectory), and to the image
//! GalaCreateProcess starts (a 32-bit program starting System32's program
//! gets SysWOW64's, as on Windows).

use crate::generic::Us64;
use crate::mem::{self, Res};
use crate::{Args, NtStatus, STATUS_SUCCESS};

/// gala's wow64.dll-only service (the kernel has none of this number):
/// GalaWow64FsRedirection(Disable, *OldValue), what the 32-bit ntdll's
/// RtlWow64EnableFsRedirectionEx calls.
pub const GALA_WOW64_FS_REDIRECTION: u32 = 0x7ffe;

/// The services this module serves, with their argument counts.
pub static SERVICES: &[(u32, usize)] = &[(GALA_WOW64_FS_REDIRECTION, 2)];

/// TEB64.TlsSlots[WOW64_TLS_FILESYSREDIR]: non-zero while redirection is
/// off.
const TEB64_REDIRECTION_SLOT: u64 = 0x1480 + 8 * 8;

/// The longest name rewritten (in UTF-16 units); longer ones pass as they
/// are.
pub const MAX_NAME: usize = 1024;

/// The file services whose names are redirected, and their
/// OBJECT_ATTRIBUTES argument.
pub static FILE_SERVICES: &[(u32, usize)] = &[
    (0x0055, 2), // NtCreateFile
    (0x0033, 2), // NtOpenFile
    (0x003d, 0), // NtQueryAttributesFile
];

fn slot() -> *mut u64 {
    let teb: u64;
    // SAFETY: GS is the running thread's 64-bit TEB.
    unsafe {
        core::arch::asm!("mov {}, gs:[0x30]", out(reg) teb, options(nostack, readonly, preserves_flags));
    }
    (teb + TEB64_REDIRECTION_SLOT) as *mut u64
}

/// Whether this thread's redirection is on.
fn enabled() -> bool {
    // SAFETY: this thread's TEB.
    unsafe { core::ptr::read_volatile(slot()) == 0 }
}

/// GalaWow64FsRedirection(Disable, *OldValue): sets this thread's flag;
/// the previous value goes to OldValue (optional).
pub fn call(_number: u32, a: &Args) -> NtStatus {
    let old = enabled_value();
    if a.p(1) != 0
        && let Err(e) = mem::put32(a.p(1), old as u32)
    {
        return e;
    }
    // SAFETY: this thread's TEB.
    unsafe { core::ptr::write_volatile(slot(), u64::from(a.u(0))) };
    STATUS_SUCCESS
}

fn enabled_value() -> u64 {
    // SAFETY: this thread's TEB.
    unsafe { core::ptr::read_volatile(slot()) }
}

/// KUSER_SHARED_DATA.NtSystemRoot (`C:\Windows`), into `out`; its length.
fn windows_dir(out: &mut [u16; 260]) -> usize {
    let mut n = 0;
    while n < out.len() {
        // SAFETY: KUSER_SHARED_DATA is mapped read-only at 0x7ffe0000 in
        // every process; NtSystemRoot (+0x30) is a NUL-terminated
        // WCHAR[260].
        let c = unsafe { core::ptr::read_volatile((0x7ffe_0030 + 2 * n) as *const u16) };
        if c == 0 {
            break;
        }
        out[n] = c;
        n += 1;
    }
    n
}

fn eq_ignore_case(a: &[u16], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(&x, &y)| x < 0x80 && (x as u8).eq_ignore_ascii_case(&y))
}

/// Whether `rest` (after a directory) starts with component path `p`
/// (backslashes inside), ending there or at a backslash.
fn starts_with_component(rest: &[u16], p: &[u8]) -> bool {
    rest.len() >= p.len()
        && eq_ignore_case(&rest[..p.len()], p)
        && (rest.len() == p.len() || rest[p.len()] == u16::from(b'\\'))
}

/// System32's subdirectories 32-bit programs see as they are.
const EXEMPT: &[&[u8]] =
    &[b"catroot", b"catroot2", b"driverstore", b"drivers\\etc", b"logfiles", b"spool"];

/// The redirected form of NT path `name` into `out` (its length), or None
/// if it is not redirected.
pub fn rewrite(name: &[u16], windir: &[u16], out: &mut [u16]) -> Option<usize> {
    const PREFIX: &[u8] = b"\\??\\";
    if name.len() < PREFIX.len() + windir.len() || !eq_ignore_case(&name[..4], PREFIX) {
        return None;
    }
    let dir = &name[4..4 + windir.len()];
    if !dir.iter().zip(windir).all(|(&a, &b)| fold(a) == fold(b)) {
        return None;
    }
    let after = &name[4 + windir.len()..];
    let head = 4 + windir.len();
    let rest = after.strip_prefix(&[u16::from(b'\\')])?;
    let (replace, with): (usize, &[u8]) = if starts_with_component(rest, b"system32") {
        let below = &rest[8..];
        let below = below.strip_prefix(&[u16::from(b'\\')]).unwrap_or(below);
        if EXEMPT.iter().any(|e| starts_with_component(below, e)) {
            return None;
        }
        (8, b"SysWOW64")
    } else if starts_with_component(rest, b"sysnative") {
        (9, b"System32")
    } else if rest.len() == 11 && eq_ignore_case(rest, b"regedit.exe") {
        (0, b"SysWOW64\\")
    } else {
        return None;
    };
    let total = name.len() - replace + with.len();
    if total > out.len() {
        return None;
    }
    let at = head + 1;
    out[..at].copy_from_slice(&name[..at]);
    for (d, &c) in out[at..].iter_mut().zip(with) {
        *d = u16::from(c);
    }
    out[at + with.len()..total].copy_from_slice(&name[at + replace..]);
    Some(total)
}

fn fold(c: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }
}

/// The redirected form of the caller's NT path at `addr` (`len` UTF-16
/// units) into `buf` (its length), if this thread's redirection is on and
/// the path is redirected.
fn redirect_at(addr: u64, len: usize, buf: &mut [u16; MAX_NAME]) -> Res<Option<usize>> {
    if addr == 0 || len > MAX_NAME || !enabled() {
        return Ok(None);
    }
    let mut name = [0u16; MAX_NAME];
    let mut raw = [0u8; 2 * MAX_NAME];
    mem::read(addr, &mut raw[..2 * len])?;
    for (i, c) in name[..len].iter_mut().enumerate() {
        *c = u16::from_le_bytes([raw[2 * i], raw[2 * i + 1]]);
    }
    let mut windir = [0u16; 260];
    let n = windows_dir(&mut windir);
    Ok(rewrite(&name[..len], &windir[..n], buf))
}

/// Redirects the name `us` (an OBJECT_ATTRIBUTES's, converted; no root
/// directory) if this thread's redirection is on and it names a
/// redirected path: `us` then points at the new name in `buf`.
pub fn apply(us: &mut Us64, root: u64, buf: &mut [u16; MAX_NAME]) -> Res<()> {
    if root != 0 {
        return Ok(());
    }
    if let Some(total) = redirect_at(us.buffer, usize::from(us.length) / 2, buf)? {
        us.length = (2 * total) as u16;
        us.maximum_length = (2 * total) as u16;
        us.buffer = buf.as_ptr() as u64;
    }
    Ok(())
}

/// GalaCreateProcess's parameter block (gala's kernel32 builds it; its
/// fields are 8 bytes whatever the caller's bitness): the image path's
/// pointer at 0, its length in characters at 8.
const IMAGE_PATH: u64 = 0;
const IMAGE_LENGTH: u64 = 8;

/// The image a 32-bit caller starts is a file it opens, redirected as its
/// other opens are: GalaCreateProcess's image path in the caller's block
/// `block` is pointed at its redirected form in `buf` for the call.
/// Returns the original (pointer, length) to put back with `restore`.
pub fn apply_image(block: u64, buf: &mut [u16; MAX_NAME]) -> Res<Option<(u64, u32)>> {
    let mut field = [0u8; 8];
    mem::read(block + IMAGE_PATH, &mut field)?;
    let path = u64::from_le_bytes(field);
    let len = mem::get32(block + IMAGE_LENGTH)?;
    let Some(total) = redirect_at(path, len as usize, buf)? else {
        return Ok(None);
    };
    mem::write(block + IMAGE_PATH, &(buf.as_ptr() as u64).to_le_bytes())?;
    mem::put32(block + IMAGE_LENGTH, total as u32)?;
    Ok(Some((path, len)))
}

/// Puts back what `apply_image` changed in the caller's block.
pub fn restore_image(block: u64, (path, len): (u64, u32)) -> Res<()> {
    mem::write(block + IMAGE_PATH, &path.to_le_bytes())?;
    mem::put32(block + IMAGE_LENGTH, len)
}
