//! What FileZilla's DLLs (wxWidgets, libfilezilla, GnuTLS, libstdc++)
//! import beyond the rest, as msvcrt.dll documents it: the wide character
//! classes (iswalpha...) and case mapping, wcstol/wcstoul/_wtoi, a few
//! string functions in the C locale (wcspbrk, wcscoll, wcsxfrm, strxfrm,
//! strncat), wcsftime, the wide environment (_wgetenv, _wputenv), wide
//! file functions (_wrename, _wremove, _wstat64, _wutime64, _wfullpath,
//! _wfindfirst64/_wfindnext64/_findclose), _filelengthi64, the stdio
//! limit (_getmaxstdio/_setmaxstdio), fgetpos/fsetpos, the wide stream
//! functions on narrow streams (UTF-8 here, as fputwc writes it),
//! _aligned_malloc/_aligned_free, _beginthread, _getdrive/_chdrive,
//! _wassert and _wperror.

use crate::heap::{free, malloc};
use crate::wide::{PATH, set_errno, utf8, wlen};
use core::ffi::c_void;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicI32, Ordering};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        sa: *mut c_void,
        disposition: u32,
        flags: u32,
        template: *mut c_void,
    ) -> *mut c_void;
    fn SetFileTime(
        h: *mut c_void,
        create: *const u64,
        access: *const u64,
        write: *const u64,
    ) -> i32;
    fn CloseHandle(h: *mut c_void) -> i32;
    fn FindFirstFileW(pattern: *const u16, data: *mut FindData) -> *mut c_void;
    fn FindNextFileW(h: *mut c_void, data: *mut FindData) -> i32;
    fn FindClose(h: *mut c_void) -> i32;
    fn GetFileSizeEx(h: *mut c_void, size: *mut i64) -> i32;
    fn GetCurrentDirectoryW(size: u32, buf: *mut u16) -> u32;
    fn SetCurrentDirectoryW(path: *const u16) -> i32;
    fn CreateThread(
        sa: *mut c_void,
        stack: usize,
        start: usize,
        arg: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> *mut c_void;
}

const EINVAL: i32 = 22;
const ENOENT: i32 = 2;
const ERANGE: i32 = 34;
const EBADF: i32 = 9;
const ENOMEM: i32 = 12;
const WEOF: u16 = 0xffff;
const INVALID_HANDLE: *mut c_void = usize::MAX as *mut c_void;

// ---- Character classes ----

use crate::wide::{ALPHA, BLANK, CONTROL, DIGIT, HEX, LOWER, PUNCT, SPACE, UPPER};

#[unsafe(no_mangle)]
pub extern "C" fn iswalpha(c: u16) -> i32 {
    crate::wide::iswctype(c, ALPHA)
}

#[unsafe(no_mangle)]
pub extern "C" fn iswalnum(c: u16) -> i32 {
    crate::wide::iswctype(c, ALPHA | DIGIT)
}

#[unsafe(no_mangle)]
pub extern "C" fn iswdigit(c: u16) -> i32 {
    crate::wide::iswctype(c, DIGIT)
}

#[unsafe(no_mangle)]
pub extern "C" fn iswxdigit(c: u16) -> i32 {
    crate::wide::iswctype(c, HEX)
}

#[unsafe(no_mangle)]
pub extern "C" fn iswspace(c: u16) -> i32 {
    crate::wide::iswctype(c, SPACE)
}

#[unsafe(no_mangle)]
pub extern "C" fn iswpunct(c: u16) -> i32 {
    crate::wide::iswctype(c, PUNCT)
}

/// iswprint: _PRINT is _BLANK | _PUNCT | _ALPHA | _DIGIT (ctype.h).
#[unsafe(no_mangle)]
pub extern "C" fn iswprint(c: u16) -> i32 {
    crate::wide::iswctype(c, BLANK | PUNCT | ALPHA | DIGIT)
}

/// towupper: ASCII letters; past Latin-1, Unicode's simple mapping (the C
/// locale maps nothing in 0x80..0xff).
#[unsafe(no_mangle)]
pub extern "C" fn towupper(c: u16) -> u16 {
    map_case(c, true)
}

#[unsafe(no_mangle)]
pub extern "C" fn towlower(c: u16) -> u16 {
    map_case(c, false)
}

fn map_case(c: u16, upper: bool) -> u16 {
    if c < 0x80 {
        let b = c as u8;
        return u16::from(if upper { b.to_ascii_uppercase() } else { b.to_ascii_lowercase() });
    }
    if c < 0x100 {
        return c;
    }
    let Some(ch) = char::from_u32(c.into()) else { return c };
    let mut mapped = [ch; 2];
    let mut n = 0;
    if upper {
        for m in ch.to_uppercase() {
            if n < 2 {
                mapped[n] = m;
            }
            n += 1;
        }
    } else {
        for m in ch.to_lowercase() {
            if n < 2 {
                mapped[n] = m;
            }
            n += 1;
        }
    }
    // Only a one-for-one mapping within the BMP.
    match (n, u16::try_from(u32::from(mapped[0]))) {
        (1, Ok(m)) => m,
        _ => c,
    }
}

// Silences "unused" for the classes this file does not name directly.
const _: u16 = UPPER | LOWER | CONTROL;

// ---- Numbers ----

/// The value, whether it overflowed, and where parsing stopped (strtol's
/// rules: spaces, a sign, `0x` for base 16 or 0, a leading 0 for octal in
/// base 0).
fn parse_wide(s: *const u16, base: i32) -> (u64, bool, bool, usize) {
    let at = |i: usize| -> u16 {
        // SAFETY: the caller's NUL-terminated string, read up to its NUL.
        unsafe { *s.add(i) }
    };
    let mut i = 0;
    while crate::wide::iswctype(at(i), SPACE) != 0 {
        i += 1;
    }
    let mut negative = false;
    if at(i) == u16::from(b'-') || at(i) == u16::from(b'+') {
        negative = at(i) == u16::from(b'-');
        i += 1;
    }
    let mut base = base as u32;
    let hex_prefix = at(i) == u16::from(b'0') && (at(i + 1) | 0x20) == u16::from(b'x');
    if (base == 0 || base == 16) && hex_prefix && (at(i + 2) as u8 as char).is_ascii_hexdigit() {
        i += 2;
        base = 16;
    } else if base == 0 {
        base = if at(i) == u16::from(b'0') { 8 } else { 10 };
    }
    if !(2..=36).contains(&base) {
        return (0, false, false, 0);
    }
    let start = i;
    let mut value: u64 = 0;
    let mut overflow = false;
    loop {
        let c = at(i);
        let d = match c {
            0x30..=0x39 => u32::from(c - 0x30),
            0x41..=0x5a => u32::from(c - 0x41 + 10),
            0x61..=0x7a => u32::from(c - 0x61 + 10),
            _ => break,
        };
        if d >= base {
            break;
        }
        value = match value.checked_mul(base.into()).and_then(|v| v.checked_add(d.into())) {
            Some(v) if v <= u64::from(u32::MAX) => v,
            _ => {
                overflow = true;
                u64::from(u32::MAX)
            }
        };
        i += 1;
    }
    if i == start {
        return (0, false, false, 0); // no digits: nothing consumed
    }
    (value, negative, overflow, i)
}

/// wcstol (a 32-bit long): LONG_MAX/LONG_MIN and ERANGE on overflow.
#[unsafe(no_mangle)]
pub extern "C" fn wcstol(s: *const u16, end: *mut *mut u16, base: i32) -> i32 {
    let (v, negative, overflow, n) = parse_wide(s, base);
    if !end.is_null() {
        // SAFETY: the caller's pointer.
        unsafe { *end = s.add(n).cast_mut() };
    }
    let limit = if negative { 1u64 << 31 } else { (1u64 << 31) - 1 };
    if overflow || v > limit {
        set_errno(ERANGE);
        return if negative { i32::MIN } else { i32::MAX };
    }
    if negative { (v as i64).wrapping_neg() as i32 } else { v as i32 }
}

/// wcstoul: ULONG_MAX and ERANGE on overflow; a minus sign negates.
#[unsafe(no_mangle)]
pub extern "C" fn wcstoul(s: *const u16, end: *mut *mut u16, base: i32) -> u32 {
    let (v, negative, overflow, n) = parse_wide(s, base);
    if !end.is_null() {
        // SAFETY: the caller's pointer.
        unsafe { *end = s.add(n).cast_mut() };
    }
    if overflow {
        set_errno(ERANGE);
        return u32::MAX;
    }
    if negative { (v as u32).wrapping_neg() } else { v as u32 }
}

#[unsafe(no_mangle)]
pub extern "C" fn _wtoi(s: *const u16) -> i32 {
    let (v, negative, _, _) = parse_wide(s, 10);
    let v = v as u32 as i32;
    if negative { v.wrapping_neg() } else { v }
}

// ---- Strings ----

#[unsafe(no_mangle)]
pub extern "C" fn wcspbrk(s: *const u16, set: *const u16) -> *mut u16 {
    // SAFETY: the caller's NUL-terminated strings.
    unsafe {
        let set = core::slice::from_raw_parts(set, wlen(set));
        let mut p = s;
        while *p != 0 {
            if set.contains(&*p) {
                return p.cast_mut();
            }
            p = p.add(1);
        }
    }
    null_mut()
}

/// wcscoll: in the C locale, wcscmp.
#[unsafe(no_mangle)]
pub extern "C" fn wcscoll(a: *const u16, b: *const u16) -> i32 {
    crate::misc::wcscmp(a, b)
}

/// wcsxfrm(dst, src, n): in the C locale, a copy when it fits; the
/// length either way.
#[unsafe(no_mangle)]
pub extern "C" fn wcsxfrm(dst: *mut u16, src: *const u16, n: usize) -> usize {
    // SAFETY: the caller's string and buffer of `n`.
    unsafe {
        let len = wlen(src);
        if len < n && !dst.is_null() {
            core::ptr::copy_nonoverlapping(src, dst, len + 1);
        }
        len
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn strxfrm(dst: *mut u8, src: *const u8, n: usize) -> usize {
    // SAFETY: as above.
    unsafe {
        let len = crate::c_str(src).len();
        if len < n && !dst.is_null() {
            core::ptr::copy_nonoverlapping(src, dst, len + 1);
        }
        len
    }
}

/// strncat(dst, src, n): at most `n` characters, then a NUL.
#[unsafe(no_mangle)]
pub extern "C" fn strncat(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    // SAFETY: the caller's strings, `dst` large enough.
    unsafe {
        let at = crate::c_str(dst).len();
        let mut i = 0;
        while i < n && *src.add(i) != 0 {
            *dst.add(at + i) = *src.add(i);
            i += 1;
        }
        *dst.add(at + i) = 0;
    }
    dst
}

/// wcsftime: strftime on the UTF-8 form of the format, widened back; the
/// characters written (without the NUL), 0 if they do not fit.
#[unsafe(no_mangle)]
pub extern "C" fn wcsftime(
    buf: *mut u16,
    max: usize,
    fmt: *const u16,
    tm: *const crate::time::Tm,
) -> usize {
    let mut f = [0u8; 3 * PATH];
    let Some(f) = utf8(fmt, &mut f) else { return 0 };
    let mut out = [0u8; 3 * PATH];
    let n = crate::time::strftime(out.as_mut_ptr(), out.len(), f, tm);
    if n == 0 {
        return 0;
    }
    let Ok(text) = core::str::from_utf8(&out[..n]) else { return 0 };
    let mut i = 0;
    for c in text.encode_utf16() {
        if i + 1 >= max {
            return 0;
        }
        // SAFETY: within the caller's `max` characters.
        unsafe { *buf.add(i) = c };
        i += 1;
    }
    // SAFETY: as above.
    unsafe { *buf.add(i) = 0 };
    i
}

// ---- The environment ----

/// _wgetenv: the value, widened into a copy kept per variable (until it
/// changes): what msvcrt's wide environment table holds.
#[unsafe(no_mangle)]
pub extern "C" fn _wgetenv(name: *const u16) -> *mut u16 {
    let mut n = [0u8; 3 * PATH];
    let Some(n) = utf8(name, &mut n) else { return null_mut() };
    let value = crate::getenv(n);
    if value.is_null() {
        return null_mut();
    }
    // SAFETY: getenv's NUL-terminated value.
    let bytes = unsafe { crate::c_str(value) };
    let text = core::str::from_utf8(bytes).unwrap_or("");
    let units = text.encode_utf16().count();
    let copy = malloc(2 * (units + 1)).cast::<u16>();
    if copy.is_null() {
        return null_mut();
    }
    for (i, c) in text.encode_utf16().enumerate() {
        // SAFETY: `units + 1` characters.
        unsafe { *copy.add(i) = c };
    }
    // SAFETY: as above.
    unsafe { *copy.add(units) = 0 };
    remember(copy);
    copy
}

/// The wide copies handed out, the oldest freed after a while (programs
/// use _wgetenv's result right away; msvcrt's own table changes with
/// _wputenv too).
fn remember(p: *mut u16) {
    static RING: [core::sync::atomic::AtomicUsize; 64] =
        [const { core::sync::atomic::AtomicUsize::new(0) }; 64];
    static NEXT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    let i = NEXT.fetch_add(1, Ordering::Relaxed) % RING.len();
    let old = RING[i].swap(p as usize, Ordering::Relaxed);
    if old != 0 {
        free(old as *mut u8);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _wputenv(entry: *const u16) -> i32 {
    let mut e = [0u8; 3 * PATH];
    match utf8(entry, &mut e) {
        Some(e) => crate::posix::_putenv(e),
        None => {
            set_errno(EINVAL);
            -1
        }
    }
}

// ---- Files ----

fn with_utf8(path: *const u16, f: impl FnOnce(*const u8) -> i32) -> i32 {
    let mut buf = [0u8; 3 * PATH];
    match utf8(path, &mut buf) {
        Some(p) => f(p),
        None => {
            set_errno(ENOENT);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _wrename(from: *const u16, to: *const u16) -> i32 {
    let mut a = [0u8; 3 * PATH];
    let Some(a) = utf8(from, &mut a) else {
        set_errno(ENOENT);
        return -1;
    };
    with_utf8(to, |b| crate::posix::rename(a, b))
}

#[unsafe(no_mangle)]
pub extern "C" fn _wremove(path: *const u16) -> i32 {
    with_utf8(path, |p| crate::posix::remove(p))
}

#[unsafe(no_mangle)]
pub extern "C" fn _wstat64(path: *const u16, buf: *mut u8) -> i32 {
    with_utf8(path, |p| crate::stdio::_stat64(p, buf))
}

/// _wfullpath(buf, path, size): through _fullpath; a malloc'd result when
/// `buf` is NULL.
#[unsafe(no_mangle)]
pub extern "C" fn _wfullpath(buf: *mut u16, path: *const u16, size: usize) -> *mut u16 {
    let mut p = [0u8; 3 * PATH];
    let Some(p) = utf8(path, &mut p) else { return null_mut() };
    let mut full = [0u8; 3 * PATH];
    if crate::posix::_fullpath(full.as_mut_ptr(), p, full.len()).is_null() {
        return null_mut();
    }
    let len = full.iter().position(|&b| b == 0).unwrap_or(0);
    let Ok(text) = core::str::from_utf8(&full[..len]) else { return null_mut() };
    let units = text.encode_utf16().count();
    let (out, room) = if buf.is_null() {
        (malloc(2 * (units + 1)).cast::<u16>(), units + 1)
    } else {
        (buf, size)
    };
    if out.is_null() {
        set_errno(ENOMEM);
        return null_mut();
    }
    if units + 1 > room {
        set_errno(ERANGE);
        return null_mut();
    }
    for (i, c) in text.encode_utf16().enumerate() {
        // SAFETY: `room` characters.
        unsafe { *out.add(i) = c };
    }
    // SAFETY: as above.
    unsafe { *out.add(units) = 0 };
    out
}

/// FILETIME (100 ns since 1601) and time64 (seconds since 1970).
const EPOCH_DIFF: i64 = 116_444_736_000_000_000;

fn time64(ft: [u32; 2]) -> i64 {
    let t = (u64::from(ft[1]) << 32 | u64::from(ft[0])) as i64;
    (t - EPOCH_DIFF) / 10_000_000
}

/// __utimbuf64 { actime, modtime }; NULL: now.
#[unsafe(no_mangle)]
pub extern "C" fn _wutime64(path: *const u16, times: *const [i64; 2]) -> i32 {
    let now = crate::misc::_time64(null_mut());
    // SAFETY: the caller's structure, if any.
    let [access, modified] = if times.is_null() { [now, now] } else { unsafe { *times } };
    let ft = |t: i64| (t * 10_000_000 + EPOCH_DIFF) as u64;
    // SAFETY: the caller's path; our handle.
    unsafe {
        // FILE_WRITE_ATTRIBUTES, share all, OPEN_EXISTING, backup semantics
        // (directories too).
        let h = CreateFileW(path, 0x100, 7, null_mut(), 3, 0x0200_0000, null_mut());
        if h == INVALID_HANDLE {
            set_errno(ENOENT);
            return -1;
        }
        let ok = SetFileTime(h, core::ptr::null(), &ft(access), &ft(modified));
        CloseHandle(h);
        if ok == 0 {
            set_errno(EINVAL);
            return -1;
        }
    }
    0
}

#[repr(C)]
pub struct FindData {
    attributes: u32,
    created: [u32; 2],
    accessed: [u32; 2],
    written: [u32; 2],
    size_high: u32,
    size_low: u32,
    reserved: [u32; 2],
    name: [u16; 260],
    short_name: [u16; 14],
}

/// struct _wfinddata64_t.
#[repr(C)]
pub struct WFindData64 {
    attrib: u32,
    time_create: i64,
    time_access: i64,
    time_write: i64,
    size: i64,
    name: [u16; 260],
}

fn fill(out: *mut WFindData64, d: &FindData) {
    // SAFETY: the caller's structure.
    unsafe {
        (*out).attrib = d.attributes;
        (*out).time_create = time64(d.created);
        (*out).time_access = time64(d.accessed);
        (*out).time_write = time64(d.written);
        (*out).size = (u64::from(d.size_high) << 32 | u64::from(d.size_low)) as i64;
        (*out).name = d.name;
    }
}

/// _wfindfirst64: a search handle (-1 and ENOENT when nothing matches).
#[unsafe(no_mangle)]
pub extern "C" fn _wfindfirst64(pattern: *const u16, out: *mut WFindData64) -> isize {
    // SAFETY: zeroed is a valid FindData; the caller's pattern.
    let mut d: FindData = unsafe { core::mem::zeroed() };
    let h = unsafe { FindFirstFileW(pattern, &mut d) };
    if h == INVALID_HANDLE {
        set_errno(ENOENT);
        return -1;
    }
    fill(out, &d);
    h as isize
}

#[unsafe(no_mangle)]
pub extern "C" fn _wfindnext64(h: isize, out: *mut WFindData64) -> i32 {
    // SAFETY: as above.
    let mut d: FindData = unsafe { core::mem::zeroed() };
    if unsafe { FindNextFileW(h as *mut c_void, &mut d) } == 0 {
        set_errno(ENOENT);
        return -1;
    }
    fill(out, &d);
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn _findclose(h: isize) -> i32 {
    // SAFETY: the caller's search handle.
    if unsafe { FindClose(h as *mut c_void) } == 0 {
        set_errno(ENOENT);
        return -1;
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn _filelengthi64(fd: i32) -> i64 {
    let h = crate::stdio::_get_osfhandle(fd);
    let mut size = 0i64;
    // SAFETY: the descriptor's handle.
    if h == -1 || unsafe { GetFileSizeEx(h as *mut c_void, &mut size) } == 0 {
        set_errno(EBADF);
        return -1;
    }
    size
}

// ---- Streams ----

/// The stdio limit: 512 by default, 20 (_IOB_ENTRIES) to 2048.
static MAX_STDIO: AtomicI32 = AtomicI32::new(512);

#[unsafe(no_mangle)]
pub extern "C" fn _getmaxstdio() -> i32 {
    MAX_STDIO.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn _setmaxstdio(n: i32) -> i32 {
    if !(20..=2048).contains(&n) {
        return -1;
    }
    MAX_STDIO.store(n, Ordering::Relaxed);
    n
}

/// fgetpos/fsetpos: fpos_t is the 64-bit offset.
#[unsafe(no_mangle)]
pub extern "C" fn fgetpos(file: *mut u8, pos: *mut i64) -> i32 {
    let at = crate::stdio::ftell(file);
    if at < 0 {
        return -1;
    }
    // SAFETY: the caller's fpos_t.
    unsafe { *pos = at.into() };
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn fsetpos(file: *mut u8, pos: *const i64) -> i32 {
    // SAFETY: the caller's fpos_t.
    let at = unsafe { *pos };
    let Ok(at) = i32::try_from(at) else {
        set_errno(EINVAL);
        return -1;
    };
    crate::stdio::fseek(file, at, 0)
}

/// fgetwc on a narrow stream: one UTF-8 character (WEOF at the end).
#[unsafe(no_mangle)]
pub extern "C" fn fgetwc(file: *mut u8) -> u16 {
    let first = crate::stdio::fgetc(file);
    if first < 0 {
        return WEOF;
    }
    let b = first as u8;
    let extra = match b {
        0xc0..=0xdf => 1,
        0xe0..=0xef => 2,
        _ => 0,
    };
    if extra == 0 {
        return b.into();
    }
    let mut bytes = [b, 0, 0];
    for slot in bytes.iter_mut().skip(1).take(extra) {
        let c = crate::stdio::fgetc(file);
        if c < 0 {
            return WEOF;
        }
        *slot = c as u8;
    }
    match core::str::from_utf8(&bytes[..=extra]).ok().and_then(|s| s.chars().next()) {
        Some(c) => u16::try_from(u32::from(c)).unwrap_or(0xfffd),
        None => 0xfffd,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn getwc(file: *mut u8) -> u16 {
    fgetwc(file)
}

/// ungetwc: an ASCII character pushes back as one byte; others are not
/// supported (WEOF).
#[unsafe(no_mangle)]
pub extern "C" fn ungetwc(c: u16, file: *mut u8) -> u16 {
    if c == WEOF || c >= 0x80 {
        return WEOF;
    }
    if crate::stdio::ungetc(c.into(), file) < 0 { WEOF } else { c }
}

#[unsafe(no_mangle)]
pub extern "C" fn putwc(c: u16, file: *mut u8) -> u16 {
    crate::stdio::fputwc(c, file)
}

#[unsafe(no_mangle)]
pub extern "C" fn fputws(s: *const u16, file: *mut u8) -> i32 {
    // SAFETY: the caller's NUL-terminated string.
    let s = unsafe { core::slice::from_raw_parts(s, wlen(s)) };
    let mut out = [0u8; 3 * PATH];
    let mut at = 0;
    for c in char::decode_utf16(s.iter().copied()) {
        let mut b = [0u8; 4];
        let e = c.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut b);
        if at + e.len() > out.len() {
            if crate::stdio::fwrite(out.as_ptr(), 1, at, file) != at {
                return -1;
            }
            at = 0;
        }
        out[at..at + e.len()].copy_from_slice(e.as_bytes());
        at += e.len();
    }
    if crate::stdio::fwrite(out.as_ptr(), 1, at, file) != at {
        return -1;
    }
    0
}

/// _putws: the string and a newline on stdout.
#[unsafe(no_mangle)]
pub extern "C" fn _putws(s: *const u16) -> i32 {
    let stdout = crate::stdio::__iob_func().wrapping_add(crate::stdio::FILE_SIZE);
    if fputws(s, stdout) < 0 || crate::stdio::fputc(i32::from(b'\n'), stdout) < 0 {
        return -1;
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn _wperror(s: *const u16) {
    let mut b = [0u8; 3 * PATH];
    crate::misc::perror(utf8(s, &mut b).unwrap_or(core::ptr::null()));
}

/// _wassert(message, file, line): as _assert (the message on stderr,
/// then abort).
#[unsafe(no_mangle)]
pub extern "C" fn _wassert(message: *const u16, file: *const u16, line: u32) -> ! {
    let mut m = [0u8; 3 * PATH];
    let mut f = [0u8; 3 * PATH];
    let m = utf8(message, &mut m).unwrap_or(b"?\0".as_ptr());
    let f = utf8(file, &mut f).unwrap_or(b"?\0".as_ptr());
    crate::_assert(m, f, line)
}

// ---- Memory and threads ----

/// _aligned_malloc(size, alignment): `alignment` a power of two; the
/// block's start is kept just below the aligned pointer for
/// _aligned_free.
#[unsafe(no_mangle)]
pub extern "C" fn _aligned_malloc(size: usize, alignment: usize) -> *mut u8 {
    if !alignment.is_power_of_two() {
        set_errno(EINVAL);
        return null_mut();
    }
    let alignment = alignment.max(8);
    let Some(total) = size.checked_add(alignment + 8) else {
        set_errno(ENOMEM);
        return null_mut();
    };
    let block = malloc(total);
    if block.is_null() {
        return null_mut();
    }
    let aligned = (block as usize + 8 + alignment - 1) & !(alignment - 1);
    // SAFETY: `aligned - 8` is inside the block (at least 8 bytes in).
    unsafe { *((aligned - 8) as *mut usize) = block as usize };
    aligned as *mut u8
}

#[unsafe(no_mangle)]
pub extern "C" fn _aligned_free(p: *mut u8) {
    if p.is_null() {
        return;
    }
    // SAFETY: an _aligned_malloc pointer: the block's start just below.
    let block = unsafe { *((p as usize - 8) as *const usize) };
    free(block as *mut u8);
}

/// What a _beginthread thread runs: the routine and its argument.
struct Start {
    routine: extern "C" fn(*mut c_void),
    arg: *mut c_void,
}

extern "system" fn begin_trampoline(p: *mut c_void) -> u32 {
    // SAFETY: the Start that _beginthread allocated for this thread.
    let (routine, arg) = unsafe {
        let s = &*(p as *const Start);
        (s.routine, s.arg)
    };
    free(p.cast());
    routine(arg);
    0
}

/// _beginthread(routine, stack, arg): the thread's handle, -1 on failure.
/// (msvcrt closes the handle itself when the thread ends; gala keeps it
/// open: a handle leak per thread, not a visible difference.)
#[unsafe(no_mangle)]
pub extern "C" fn _beginthread(routine: usize, stack: u32, arg: *mut c_void) -> usize {
    if routine == 0 {
        set_errno(EINVAL);
        return usize::MAX;
    }
    let start = malloc(size_of::<Start>()).cast::<Start>();
    if start.is_null() {
        set_errno(ENOMEM);
        return usize::MAX;
    }
    // SAFETY: a fresh block; `routine` is the caller's function.
    unsafe {
        start.write(Start {
            routine: core::mem::transmute::<usize, extern "C" fn(*mut c_void)>(routine),
            arg,
        });
        let h = CreateThread(
            null_mut(),
            stack as usize,
            begin_trampoline as *const () as usize,
            start.cast(),
            0,
            null_mut(),
        );
        if h.is_null() {
            free(start.cast());
            return usize::MAX;
        }
        h as usize
    }
}

/// _getdrive: the current drive, 1 for A:.
#[unsafe(no_mangle)]
pub extern "C" fn _getdrive() -> i32 {
    let mut buf = [0u16; PATH];
    // SAFETY: our buffer.
    let n = unsafe { GetCurrentDirectoryW(buf.len() as u32, buf.as_mut_ptr()) };
    if n < 2 || buf[1] != u16::from(b':') {
        return 0;
    }
    i32::from((buf[0] as u8).to_ascii_uppercase()) - i32::from(b'A') + 1
}

/// _chdrive(drive): makes drive 1..26 (A: to Z:) current; 0 or -1.
#[unsafe(no_mangle)]
pub extern "C" fn _chdrive(drive: i32) -> i32 {
    if !(1..=26).contains(&drive) {
        set_errno(EINVAL);
        return -1;
    }
    let path = [u16::from(b'A' + (drive - 1) as u8), u16::from(b':'), 0];
    // SAFETY: a NUL-terminated path.
    if unsafe { SetCurrentDirectoryW(path.as_ptr()) } == 0 {
        set_errno(EACCES_OR_NOENT);
        return -1;
    }
    0
}

const EACCES_OR_NOENT: i32 = 13;
