//! What MinGit's git.exe imports beyond the rest: the wide string and
//! path functions (paths go through the narrow ones, as UTF-8, the ANSI
//! code page here), the wide startup (`__wgetmainargs`, `__winitenv`,
//! `_wpgmptr`), msvcrt's own `swprintf` (no count), `fscanf`, `_execl`
//! and a few small ones, as msvcrt.dll documents them.

use crate::heap::malloc;
use core::ffi::c_void;
use core::ptr::null_mut;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetCurrentDirectoryW(size: u32, buf: *mut u16) -> u32;
    fn GetModuleFileNameW(module: *mut c_void, buf: *mut u16, size: u32) -> u32;
    fn FlushFileBuffers(h: *mut c_void) -> i32;
}

/// Characters of a path (on the stack).
pub(crate) const PATH: usize = 520;

pub(crate) fn set_errno(e: i32) {
    // SAFETY: _errno() points to the process's errno.
    unsafe { *crate::stdio::_errno() = e };
}

/// The length of a NUL-terminated wide string.
///
/// # Safety
/// `s` is a valid NUL-terminated string.
pub(crate) unsafe fn wlen(s: *const u16) -> usize {
    let mut n = 0;
    // SAFETY: up to the terminator.
    while unsafe { *s.add(n) } != 0 {
        n += 1;
    }
    n
}

/// A NUL-terminated wide string as NUL-terminated UTF-8 in `out`.
pub(crate) fn utf8(w: *const u16, out: &mut [u8]) -> Option<*const u8> {
    if w.is_null() {
        return None;
    }
    // SAFETY: the caller's NUL-terminated string.
    let s = unsafe { core::slice::from_raw_parts(w, wlen(w)) };
    let mut at = 0;
    for c in char::decode_utf16(s.iter().copied()) {
        let mut b = [0u8; 4];
        let e = c.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut b);
        out.get_mut(at..at + e.len())?.copy_from_slice(e.as_bytes());
        at += e.len();
    }
    *out.get_mut(at)? = 0;
    Some(out.as_ptr())
}

/// Runs `f` on the UTF-8 form of `path` (ENOENT if it has none).
fn narrow_path(path: *const u16, f: impl FnOnce(*const u8) -> i32) -> i32 {
    let mut buf = [0u8; 4 * PATH];
    match utf8(path, &mut buf) {
        Some(p) => f(p),
        None => {
            set_errno(2); // ENOENT
            -1
        }
    }
}

// ---- Strings ----

#[unsafe(no_mangle)]
pub extern "C" fn wcscat(dst: *mut u16, src: *const u16) -> *mut u16 {
    // SAFETY: the caller's NUL-terminated strings, `dst` large enough.
    unsafe {
        let (d, s) = (wlen(dst), wlen(src));
        core::ptr::copy(src, dst.add(d), s + 1);
    }
    dst
}

/// wcsncat(dst, src, n): at most `n` characters of `src`, then a NUL.
#[unsafe(no_mangle)]
pub extern "C" fn wcsncat(dst: *mut u16, src: *const u16, n: usize) -> *mut u16 {
    // SAFETY: as wcscat.
    unsafe {
        let d = wlen(dst);
        let mut i = 0;
        while i < n && *src.add(i) != 0 {
            *dst.add(d + i) = *src.add(i);
            i += 1;
        }
        *dst.add(d + i) = 0;
    }
    dst
}

/// wcsncpy(dst, src, n): `src`, padded with NULs to `n` characters (no
/// terminator if `src` is that long).
#[unsafe(no_mangle)]
pub extern "C" fn wcsncpy(dst: *mut u16, src: *const u16, n: usize) -> *mut u16 {
    // SAFETY: `dst` holds `n` characters; `src` is NUL-terminated.
    unsafe {
        let mut i = 0;
        while i < n && *src.add(i) != 0 {
            *dst.add(i) = *src.add(i);
            i += 1;
        }
        while i < n {
            *dst.add(i) = 0;
            i += 1;
        }
    }
    dst
}

/// wcsspn(s, accept): the length of the prefix of `s` made of `accept`'s
/// characters.
#[unsafe(no_mangle)]
pub extern "C" fn wcsspn(s: *const u16, accept: *const u16) -> usize {
    // SAFETY: NUL-terminated strings.
    unsafe {
        let set = core::slice::from_raw_parts(accept, wlen(accept));
        let mut n = 0;
        while *s.add(n) != 0 && set.contains(&*s.add(n)) {
            n += 1;
        }
        n
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _wcsdup(s: *const u16) -> *mut u16 {
    if s.is_null() {
        return null_mut();
    }
    // SAFETY: a NUL-terminated string; the copy has room for it.
    unsafe {
        let n = wlen(s) + 1;
        let out = malloc(2 * n).cast::<u16>();
        if !out.is_null() {
            core::ptr::copy_nonoverlapping(s, out, n);
        }
        out
    }
}

/// msvcrt.dll's swprintf(buf, fmt, ...) has no count (the C standard's
/// is `_snwprintf`'s shape): the arguments as a va_list (below).
#[unsafe(no_mangle)]
pub extern "C" fn _gala_vswprintf(buf: *mut u16, fmt: *const u16, args: *const u64) -> i32 {
    crate::stdio::_vsnwprintf(buf, usize::MAX / 4, fmt, args)
}

/// atol: atoi's value (long is 32 bits).
#[unsafe(no_mangle)]
pub extern "C" fn atol(s: *const u8) -> i32 {
    crate::misc::atoi(s)
}

// iswctype's classes (ctype.h).
pub(crate) const UPPER: u16 = 0x1;
pub(crate) const LOWER: u16 = 0x2;
pub(crate) const DIGIT: u16 = 0x4;
pub(crate) const SPACE: u16 = 0x8;
pub(crate) const PUNCT: u16 = 0x10;
pub(crate) const CONTROL: u16 = 0x20;
pub(crate) const BLANK: u16 = 0x40;
pub(crate) const HEX: u16 = 0x80;
pub(crate) const ALPHA: u16 = 0x100;

/// iswctype(c, mask): whether `c` is in any of the classes: ASCII by
/// msvcrt's table (_BLANK only for the space), the rest by Unicode's
/// properties (letters, numbers, white space, controls; other printable
/// characters are punctuation), as msvcrt classifies through
/// GetStringTypeW.
#[unsafe(no_mangle)]
pub extern "C" fn iswctype(c: u16, mask: u16) -> i32 {
    i32::from(classes(c) & mask)
}

pub(crate) fn classes(c: u16) -> u16 {
    let mut classes = 0;
    if c < 0x80 {
        let b = c as u8 as char;
        if b.is_ascii_uppercase() {
            classes |= UPPER | ALPHA;
        }
        if b.is_ascii_lowercase() {
            classes |= LOWER | ALPHA;
        }
        if b.is_ascii_digit() {
            classes |= DIGIT;
        }
        if b.is_ascii_hexdigit() {
            classes |= HEX;
        }
        if matches!(b, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r') {
            classes |= SPACE;
        }
        if b == ' ' {
            classes |= BLANK;
        }
        if b.is_ascii_punctuation() {
            classes |= PUNCT;
        }
        if b.is_ascii_control() {
            classes |= CONTROL;
        }
        return classes;
    }
    let Some(ch) = char::from_u32(c.into()) else { return 0 };
    if ch.is_alphabetic() {
        classes |= ALPHA;
        if ch.is_uppercase() {
            classes |= UPPER;
        }
        if ch.is_lowercase() {
            classes |= LOWER;
        }
    } else if ch.is_control() {
        classes |= CONTROL;
    } else if ch.is_whitespace() {
        classes |= SPACE | BLANK;
    } else if !ch.is_numeric() {
        classes |= PUNCT;
    }
    classes
}

// ---- Paths ----

#[unsafe(no_mangle)]
pub extern "C" fn _waccess(path: *const u16, mode: i32) -> i32 {
    narrow_path(path, |p| crate::posix::_access(p, mode))
}

#[unsafe(no_mangle)]
pub extern "C" fn _wmkdir(path: *const u16) -> i32 {
    narrow_path(path, |p| crate::posix::_mkdir(p))
}

#[unsafe(no_mangle)]
pub extern "C" fn _wrmdir(path: *const u16) -> i32 {
    narrow_path(path, |p| crate::posix::_rmdir(p))
}

#[unsafe(no_mangle)]
pub extern "C" fn _wunlink(path: *const u16) -> i32 {
    narrow_path(path, |p| crate::posix::_unlink(p))
}

#[unsafe(no_mangle)]
pub extern "C" fn _wchdir(path: *const u16) -> i32 {
    narrow_path(path, |p| crate::posix::_chdir(p))
}

#[unsafe(no_mangle)]
pub extern "C" fn _wchmod(path: *const u16, mode: i32) -> i32 {
    narrow_path(path, |p| crate::posix::_chmod(p, mode))
}

/// _wopen(path, flags, mode).
#[unsafe(no_mangle)]
pub extern "C" fn _wopen(path: *const u16, flags: i32, mode: i32) -> i32 {
    narrow_path(path, |p| crate::stdio::_open(p, flags, mode))
}

/// _wgetcwd(buf, size): `size` characters; with a null buffer, a
/// malloc'ed one (at least `size`, or as long as needed).
#[unsafe(no_mangle)]
pub extern "C" fn _wgetcwd(buf: *mut u16, size: i32) -> *mut u16 {
    let mut w = [0u16; PATH];
    // SAFETY: a buffer of PATH characters.
    let n = unsafe { GetCurrentDirectoryW(PATH as u32, w.as_mut_ptr()) } as usize;
    if n == 0 || n >= PATH {
        set_errno(2);
        return null_mut();
    }
    let (out, cap) = if buf.is_null() {
        let cap = (size.max(0) as usize).max(n + 1);
        (malloc(2 * cap).cast::<u16>(), cap)
    } else {
        (buf, size.max(0) as usize)
    };
    if out.is_null() {
        set_errno(12); // ENOMEM
        return null_mut();
    }
    if n + 1 > cap {
        set_errno(34); // ERANGE
        return null_mut();
    }
    // SAFETY: `cap` characters at `out`.
    unsafe {
        core::ptr::copy_nonoverlapping(w.as_ptr(), out, n);
        *out.add(n) = 0;
    }
    out
}

/// _wmktemp(template): as _mktemp, on a wide template.
#[unsafe(no_mangle)]
pub extern "C" fn _wmktemp(template: *mut u16) -> *mut u16 {
    // SAFETY: the caller's NUL-terminated, writable template.
    let t = unsafe { core::slice::from_raw_parts_mut(template, wlen(template)) };
    if t.len() < 6 || t[t.len() - 6..].iter().any(|&c| c != u16::from(b'X')) {
        set_errno(22); // EINVAL
        return null_mut();
    }
    let start = t.len() - 6;
    let mut pid = crate::posix::_getpid() as u32;
    for i in (start + 1..t.len()).rev() {
        t[i] = u16::from(b'0') + (pid % 10) as u16;
        pid /= 10;
    }
    for letter in b'a'..=b'z' {
        t[start] = u16::from(letter);
        if _waccess(template, 0) != 0 {
            return template;
        }
    }
    set_errno(17); // EEXIST
    null_mut()
}

/// _wfreopen(path, mode, file).
#[unsafe(no_mangle)]
pub extern "C" fn _wfreopen(path: *const u16, mode: *const u16, file: *mut u8) -> *mut u8 {
    // Under 4 KiB of stack together (no __chkstk here).
    let (mut p, mut m) = ([0u8; 3 * PATH], [0u8; 32]);
    match (utf8(path, &mut p), utf8(mode, &mut m)) {
        (Some(p), Some(m)) => crate::stdio::freopen(p, m, file),
        _ => {
            set_errno(22);
            null_mut()
        }
    }
}

// ---- Startup ----

/// __winitenv: the environment main got (set by __wgetmainargs).
#[unsafe(no_mangle)]
pub static mut __winitenv: *mut *mut u16 = null_mut();
/// _wpgmptr: the program's path (set by __wgetmainargs).
#[unsafe(no_mangle)]
pub static mut _wpgmptr: *mut u16 = null_mut();
/// __wargv / __argc as the wide startup sets them.
#[unsafe(no_mangle)]
pub static mut __wargv: *mut *mut u16 = null_mut();

static mut WARGC: i32 = 0;
static mut WARGV: *mut *mut u16 = null_mut();
static WSTARTUP: crate::Lock = crate::Lock::new();

/// A heap copy of `s` with a NUL.
fn wcopy(s: &[u16]) -> *mut u16 {
    let out = malloc(2 * (s.len() + 1)).cast::<u16>();
    if !out.is_null() {
        // SAFETY: room for `s` and the terminator.
        unsafe {
            core::ptr::copy_nonoverlapping(s.as_ptr(), out, s.len());
            *out.add(s.len()) = 0;
        }
    }
    out
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetCommandLineW() -> *const u16;
    fn GetEnvironmentStringsW() -> *mut u16;
    fn FreeEnvironmentStringsW(block: *mut u16) -> i32;
}

fn build_wide() {
    // SAFETY: the process's command line and an environment block copy
    // (freed below); our own arrays, sized before they are filled.
    unsafe {
        let p = GetCommandLineW();
        let line = core::slice::from_raw_parts(p, wlen(p));
        let mut count = 0usize;
        crate::split_args(line, |_| count += 1);
        let argv = malloc((count + 1) * 8).cast::<*mut u16>();
        if argv.is_null() {
            return;
        }
        let mut i = 0;
        crate::split_args(line, |a| {
            *argv.add(i) = wcopy(a);
            i += 1;
        });
        *argv.add(count) = null_mut();
        WARGC = count as i32;
        WARGV = argv;

        let block = GetEnvironmentStringsW();
        if !block.is_null() {
            let mut entries = 0;
            let mut q = block;
            while *q != 0 {
                q = q.add(wlen(q) + 1);
                entries += 1;
            }
            let env = malloc((entries + 1) * 8).cast::<*mut u16>();
            if !env.is_null() {
                let (mut q, mut n) = (block, 0);
                while *q != 0 {
                    let entry = core::slice::from_raw_parts(q, wlen(q));
                    if entry.first() != Some(&u16::from(b'=')) {
                        *env.add(n) = wcopy(entry);
                        n += 1;
                    }
                    q = q.add(entry.len() + 1);
                }
                *env.add(n) = null_mut();
                __winitenv = env;
            }
            FreeEnvironmentStringsW(block);
        }

        let mut path = [0u16; PATH];
        let n = GetModuleFileNameW(null_mut(), path.as_mut_ptr(), PATH as u32) as usize;
        _wpgmptr = wcopy(&path[..n.min(PATH)]);
    }
}

/// __wgetmainargs(&argc, &wargv, &wenvp, dowildcard, &startinfo): the
/// wide arguments and environment for wmain (built once).
#[unsafe(no_mangle)]
pub extern "C" fn __wgetmainargs(
    argc: *mut i32,
    argv: *mut *mut *mut u16,
    envp: *mut *mut *mut u16,
    _wildcard: i32,
    _startinfo: *mut c_void,
) -> i32 {
    WSTARTUP.with(|| {
        // SAFETY: the runtime's own statics.
        if unsafe { WARGV.is_null() } {
            build_wide();
        }
    });
    // SAFETY: the caller's output pointers; the statics are set.
    unsafe {
        *argc = WARGC;
        *argv = WARGV;
        __wargv = WARGV;
        crate::__argc = WARGC;
        *envp = __winitenv;
    }
    0
}

// ---- Streams and descriptors ----

/// _commit(fd): flushes the file to the disk.
#[unsafe(no_mangle)]
pub extern "C" fn _commit(fd: i32) -> i32 {
    let h = crate::stdio::_get_osfhandle(fd);
    // SAFETY: the descriptor's handle.
    if h == -1 || unsafe { FlushFileBuffers(h as *mut c_void) } == 0 {
        set_errno(9); // EBADF
        return -1;
    }
    0
}

/// _flushall: streams are unbuffered here; the number of open streams is
/// not tracked, so 0.
#[unsafe(no_mangle)]
pub extern "C" fn _flushall() -> i32 {
    0
}

/// rewind(file): to the start, error and end-of-file indicators cleared.
#[unsafe(no_mangle)]
pub extern "C" fn rewind(file: *mut u8) {
    crate::stdio::fseek(file, 0, 0);
    crate::stdio::clearerr(file);
}

/// getchar: fgetc(stdin).
#[unsafe(no_mangle)]
pub extern "C" fn getchar() -> i32 {
    crate::stdio::fgetc(crate::stdio::__iob_func())
}

/// vfscanf on a line at a time: reads one line (up to and including its
/// newline) and scans it. Conversions that span lines are not supported;
/// what a line's conversions leave unread is dropped.
#[unsafe(no_mangle)]
pub extern "C" fn _gala_vfscanf(file: *mut u8, format: *const u8, args: *const u64) -> i32 {
    let mut line = [0u8; 1024];
    loop {
        if crate::stdio::fgets(line.as_mut_ptr(), line.len() as i32, file).is_null() {
            return -1; // EOF
        }
        // A blank line only feeds the format's leading whitespace.
        if line.iter().take_while(|&&b| b != 0).any(|b| !b.is_ascii_whitespace()) {
            break;
        }
    }
    crate::scan::vsscanf(line.as_ptr(), format, args)
}

/// _execv(path, argv): runs the program and ends this process (msvcrt
/// starts the new process and exits; the caller never returns).
#[unsafe(no_mangle)]
pub extern "C" fn _execv(path: *const u8, argv: *const *const u8) -> isize {
    // SAFETY: the runtime's environment array.
    let env = unsafe { crate::_environ }.cast_const().cast();
    let h = crate::posix::_spawnve(1, path, argv, env); // _P_NOWAIT
    if h == -1 {
        return -1;
    }
    crate::_exit(0)
}

/// _execvp: _execv with the program searched for in PATH.
#[unsafe(no_mangle)]
pub extern "C" fn _execvp(path: *const u8, argv: *const *const u8) -> isize {
    if crate::posix::_spawnvp(1, path, argv) == -1 {
        return -1;
    }
    crate::_exit(0)
}

// swprintf(buf, fmt, ...) and fscanf(file, fmt, ...): sprintf's shape (see
// stdio.rs). _execl(path, arg0, ...) and _execlp: the arguments after the
// path, spilled to the home area next to the stack ones, are the NULL-
// terminated argv.
core::arch::global_asm!(
    ".globl swprintf",
    "swprintf:",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "lea r8, [rsp+24]",
    "sub rsp, 40",
    "call {vswprintf}",
    "add rsp, 40",
    "ret",
    ".globl fscanf",
    "fscanf:",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "lea r8, [rsp+24]",
    "sub rsp, 40",
    "call {vfscanf}",
    "add rsp, 40",
    "ret",
    ".globl _execl",
    "_execl:",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "lea rdx, [rsp+16]",
    "sub rsp, 40",
    "call {execv}",
    "add rsp, 40",
    "ret",
    ".globl _execlp",
    "_execlp:",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "lea rdx, [rsp+16]",
    "sub rsp, 40",
    "call {execvp}",
    "add rsp, 40",
    "ret",
    vswprintf = sym _gala_vswprintf,
    vfscanf = sym _gala_vfscanf,
    execv = sym _execv,
    execvp = sym _execvp,
);
