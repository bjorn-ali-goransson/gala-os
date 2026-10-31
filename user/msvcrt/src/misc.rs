//! The rest of the C library OpenSSL needs: string copies and searches,
//! number parsing (strtol and friends), wide strings, the "C" locale
//! queries, strerror, raise, _time64 and the runtime's locks.

use crate::heap::malloc;
use core::ptr::null_mut;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetSystemTimeAsFileTime(time: *mut u64);
}

/// The bytes of a NUL-terminated string.
///
/// # Safety
/// `s` is NUL-terminated.
unsafe fn bytes<'a>(s: *const u8) -> &'a [u8] {
    // SAFETY: forwarded.
    unsafe { crate::c_str(s) }
}

#[unsafe(no_mangle)]
pub extern "C" fn strcpy(dst: *mut u8, src: *const u8) -> *mut u8 {
    // SAFETY: the caller's string and a buffer that holds it.
    unsafe {
        let s = bytes(src);
        core::ptr::copy(s.as_ptr(), dst, s.len());
        *dst.add(s.len()) = 0;
    }
    dst
}

/// strncpy: at most `n` bytes, zero-padded, not terminated if `src` fills it.
#[unsafe(no_mangle)]
pub extern "C" fn strncpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    // SAFETY: the caller's string and `n`-byte buffer.
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

#[unsafe(no_mangle)]
pub extern "C" fn strcat(dst: *mut u8, src: *const u8) -> *mut u8 {
    // SAFETY: the caller's strings; `dst` has room for both.
    unsafe { strcpy(dst.add(crate::strlen(dst)), src) };
    dst
}

#[unsafe(no_mangle)]
pub extern "C" fn strrchr(s: *const u8, c: i32) -> *mut u8 {
    let c = c as u8;
    // SAFETY: the caller's NUL-terminated string (its terminator matches 0).
    unsafe {
        let b = bytes(s);
        if c == 0 {
            return s.add(b.len()).cast_mut();
        }
        b.iter().rposition(|&x| x == c).map_or(null_mut(), |i| s.add(i).cast_mut())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn strspn(s: *const u8, accept: *const u8) -> usize {
    // SAFETY: the caller's NUL-terminated strings.
    unsafe {
        let a = bytes(accept);
        bytes(s).iter().take_while(|c| a.contains(c)).count()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn strcspn(s: *const u8, reject: *const u8) -> usize {
    // SAFETY: the caller's NUL-terminated strings.
    unsafe {
        let r = bytes(reject);
        bytes(s).iter().take_while(|c| !r.contains(c)).count()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _strdup(s: *const u8) -> *mut u8 {
    // SAFETY: the caller's NUL-terminated string.
    let n = unsafe { bytes(s).len() };
    let p = malloc(n + 1);
    if !p.is_null() {
        // SAFETY: n + 1 bytes each.
        unsafe { core::ptr::copy_nonoverlapping(s, p, n + 1) };
    }
    p
}

#[unsafe(no_mangle)]
pub extern "C" fn isspace(c: i32) -> i32 {
    i32::from(matches!(c, 0x20 | 0x09..=0x0d))
}

#[unsafe(no_mangle)]
pub extern "C" fn isxdigit(c: i32) -> i32 {
    i32::from(u8::try_from(c).is_ok_and(|b| b.is_ascii_hexdigit()))
}

/// The C rules of strtol: white space, a sign, a base prefix (0x for 16,
/// 0 for 8 when `base` is 0), digits; `end` past the last one used.
/// Returns (negative, magnitude, overflowed).
fn parse(s: *const u8, end: *mut *mut u8, base: i32, limit: u64) -> (bool, u64, bool) {
    // SAFETY: the caller's NUL-terminated string and optional end pointer.
    unsafe {
        let b = bytes(s);
        let mut i = 0;
        while i < b.len() && isspace(i32::from(b[i])) != 0 {
            i += 1;
        }
        let negative = b.get(i) == Some(&b'-');
        if matches!(b.get(i), Some(b'-' | b'+')) {
            i += 1;
        }
        let mut base = base as u64;
        let hex_prefix = b.get(i) == Some(&b'0') && matches!(b.get(i + 1), Some(b'x' | b'X'));
        if (base == 0 || base == 16)
            && hex_prefix
            && b.get(i + 2).is_some_and(u8::is_ascii_hexdigit)
        {
            i += 2;
            base = 16;
        } else if base == 0 {
            base = if b.get(i) == Some(&b'0') { 8 } else { 10 };
        }
        let start = i;
        let (mut v, mut over) = (0u64, false);
        while let Some(d) = b.get(i).and_then(|&c| (c as char).to_digit(36)) {
            if u64::from(d) >= base {
                break;
            }
            match v.checked_mul(base).and_then(|x| x.checked_add(u64::from(d))) {
                Some(x) if x <= limit => v = x,
                _ => over = true,
            }
            i += 1;
        }
        if !end.is_null() {
            *end = if i == start { s.cast_mut() } else { s.add(i).cast_mut() };
        }
        (negative, v, over)
    }
}

const ERANGE: i32 = 34;

#[unsafe(no_mangle)]
pub extern "C" fn _strtoi64(s: *const u8, end: *mut *mut u8, base: i32) -> i64 {
    let (negative, v, over) = parse(s, end, base, u64::MAX);
    if negative {
        if over || v > i64::MAX as u64 + 1 {
            // SAFETY: errno.
            unsafe { *crate::stdio::_errno() = ERANGE };
            return i64::MIN;
        }
        (v as i64).wrapping_neg()
    } else if over || v > i64::MAX as u64 {
        // SAFETY: errno.
        unsafe { *crate::stdio::_errno() = ERANGE };
        i64::MAX
    } else {
        v as i64
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _strtoui64(s: *const u8, end: *mut *mut u8, base: i32) -> u64 {
    let (negative, v, over) = parse(s, end, base, u64::MAX);
    if over {
        // SAFETY: errno.
        unsafe { *crate::stdio::_errno() = ERANGE };
        return u64::MAX;
    }
    if negative { v.wrapping_neg() } else { v }
}

/// strtol (`long` is 32-bit on Windows).
#[unsafe(no_mangle)]
pub extern "C" fn strtol(s: *const u8, end: *mut *mut u8, base: i32) -> i32 {
    _strtoi64(s, end, base).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn strtoul(s: *const u8, end: *mut *mut u8, base: i32) -> u32 {
    let (negative, v, over) = parse(s, end, base, u64::from(u32::MAX));
    if over {
        // SAFETY: errno.
        unsafe { *crate::stdio::_errno() = ERANGE };
        return u32::MAX;
    }
    if negative { (v as u32).wrapping_neg() } else { v as u32 }
}

#[unsafe(no_mangle)]
pub extern "C" fn atoi(s: *const u8) -> i32 {
    strtol(s, null_mut(), 10)
}

// ---- Wide strings ---------------------------------------------------------------

fn wlen(s: *const u16) -> usize {
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated wide string.
    unsafe {
        while *s.add(n) != 0 {
            n += 1;
        }
    }
    n
}

#[unsafe(no_mangle)]
pub extern "C" fn wcslen(s: *const u16) -> usize {
    wlen(s)
}

#[unsafe(no_mangle)]
pub extern "C" fn wcscmp(a: *const u16, b: *const u16) -> i32 {
    // SAFETY: the caller's NUL-terminated wide strings.
    unsafe {
        let mut i = 0;
        loop {
            let (x, y) = (*a.add(i), *b.add(i));
            if x != y {
                return i32::from(x) - i32::from(y);
            }
            if x == 0 {
                return 0;
            }
            i += 1;
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn wcscpy(dst: *mut u16, src: *const u16) -> *mut u16 {
    // SAFETY: the caller's string and a buffer that holds it.
    unsafe { core::ptr::copy(src, dst, wlen(src) + 1) };
    dst
}

#[unsafe(no_mangle)]
pub extern "C" fn wcsstr(haystack: *const u16, needle: *const u16) -> *mut u16 {
    // SAFETY: the caller's NUL-terminated wide strings.
    unsafe {
        let h = core::slice::from_raw_parts(haystack, wlen(haystack));
        let n = core::slice::from_raw_parts(needle, wlen(needle));
        if n.is_empty() {
            return haystack.cast_mut();
        }
        h.windows(n.len()).position(|w| w == n).map_or(null_mut(), |i| haystack.add(i).cast_mut())
    }
}

/// wcstombs(dst, src, n): UTF-16 to the multibyte code page (UTF-8 here);
/// without `dst`, the length needed.
#[unsafe(no_mangle)]
pub extern "C" fn wcstombs(dst: *mut u8, src: *const u16, n: usize) -> usize {
    // SAFETY: the caller's NUL-terminated wide string.
    let s = unsafe { core::slice::from_raw_parts(src, wlen(src)) };
    let mut at = 0;
    for c in char::decode_utf16(s.iter().copied()) {
        let mut b = [0u8; 4];
        let e = c.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut b);
        if !dst.is_null() {
            if at + e.len() > n {
                break;
            }
            // SAFETY: within the caller's `n` bytes.
            unsafe { core::ptr::copy_nonoverlapping(e.as_ptr(), dst.add(at), e.len()) };
        }
        at += e.len();
    }
    if !dst.is_null() && at < n {
        // SAFETY: room for the terminator.
        unsafe { *dst.add(at) = 0 };
    }
    at
}

// ---- Locale, errors, signals, time, locks -------------------------------------------

/// struct lconv of the "C" locale: ten string fields ("." and empty), then
/// the CHAR_MAX fields; zero-filled after (later CRT versions' wide fields).
#[repr(C)]
pub struct Lconv {
    strings: [*const u8; 10],
    chars: [u8; 8],
    rest: [usize; 8],
}

static mut LCONV: Lconv = Lconv { strings: [core::ptr::null(); 10], chars: [127; 8], rest: [0; 8] };

#[unsafe(no_mangle)]
pub extern "C" fn localeconv() -> *mut Lconv {
    // SAFETY: our static; the strings are 'static.
    unsafe {
        let l = &mut *(&raw mut LCONV);
        l.strings = [b"\0".as_ptr(); 10];
        l.strings[0] = b".\0".as_ptr();
        &raw mut LCONV
    }
}

/// ___lc_codepage_func: the "C" locale's code page (none: 0).
#[unsafe(no_mangle)]
pub extern "C" fn ___lc_codepage_func() -> u32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ___mb_cur_max_func() -> i32 {
    1
}

/// strerror: the CRT's texts for the common errno values.
#[unsafe(no_mangle)]
pub extern "C" fn strerror(e: i32) -> *const u8 {
    let text: &[u8] = match e {
        0 => b"No error\0",
        1 => b"Operation not permitted\0",
        2 => b"No such file or directory\0",
        5 => b"Input/output error\0",
        9 => b"Bad file descriptor\0",
        12 => b"Not enough space\0",
        13 => b"Permission denied\0",
        17 => b"File exists\0",
        22 => b"Invalid argument\0",
        24 => b"Too many open files\0",
        28 => b"No space left on device\0",
        34 => b"Result too large\0",
        _ => b"Unknown error\0",
    };
    text.as_ptr()
}

/// raise(sig): no signal handlers run on gala; SIGABRT (22) and the rest
/// end the process like an unhandled signal (exit code 3).
#[unsafe(no_mangle)]
pub extern "C" fn raise(sig: i32) -> i32 {
    crate::missing(b"msvcrt.dll!raise", sig as u64);
    crate::_exit(3)
}

/// _time64: seconds since 1970 (and stored if `t` is given).
#[unsafe(no_mangle)]
pub extern "C" fn _time64(t: *mut i64) -> i64 {
    let mut ft = 0u64;
    // SAFETY: a valid output pointer.
    unsafe { GetSystemTimeAsFileTime(&mut ft) };
    let now = (ft / 10_000_000) as i64 - 11_644_473_600;
    if !t.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { *t = now };
    }
    now
}

/// _lock/_unlock: the CRT's internal locks. gala's runtime keeps its own
/// (heap, exit list), so these have nothing to guard.
#[unsafe(no_mangle)]
pub extern "C" fn _lock(_which: i32) {}

#[unsafe(no_mangle)]
pub extern "C" fn _unlock(_which: i32) {}

// ---- Character classes (the "C" locale: ASCII) ----

macro_rules! ctype {
    ($($name:ident: $test:expr;)*) => {$(
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(c: i32) -> i32 {
            let f: fn(u8) -> bool = $test;
            i32::from(u8::try_from(c).is_ok_and(f))
        }
    )*};
}

ctype! {
    isalnum: |b| b.is_ascii_alphanumeric();
    isalpha: |b| b.is_ascii_alphabetic();
    iscntrl: |b| b.is_ascii_control();
    isdigit: |b| b.is_ascii_digit();
    isgraph: |b| b.is_ascii_graphic();
    islower: |b| b.is_ascii_lowercase();
    isprint: |b| b.is_ascii_graphic() || b == b' ';
    ispunct: |b| b.is_ascii_punctuation();
    isupper: |b| b.is_ascii_uppercase();
}

// ---- Strings ----

/// Compares at most `n` bytes ignoring ASCII case.
fn compare_ci(a: *const u8, b: *const u8, n: usize) -> i32 {
    for i in 0..n {
        // SAFETY: the caller's NUL-terminated strings (we stop at a NUL).
        let (x, y) = unsafe { (*a.add(i), *b.add(i)) };
        let (x, y) = (x.to_ascii_lowercase(), y.to_ascii_lowercase());
        if x != y || x == 0 {
            return i32::from(x) - i32::from(y);
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn _stricmp(a: *const u8, b: *const u8) -> i32 {
    compare_ci(a, b, usize::MAX)
}

#[unsafe(no_mangle)]
pub extern "C" fn _strnicmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    compare_ci(a, b, n)
}

#[unsafe(no_mangle)]
pub extern "C" fn wcsncmp(a: *const u16, b: *const u16, n: usize) -> i32 {
    for i in 0..n {
        // SAFETY: the caller's NUL-terminated strings.
        let (x, y) = unsafe { (*a.add(i), *b.add(i)) };
        if x != y || x == 0 {
            return i32::from(x) - i32::from(y);
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn _wcsnicmp(a: *const u16, b: *const u16, n: usize) -> i32 {
    let lower = |c: u16| if (0x41..=0x5a).contains(&c) { c + 32 } else { c };
    for i in 0..n {
        // SAFETY: the caller's NUL-terminated strings.
        let (x, y) = unsafe { (lower(*a.add(i)), lower(*b.add(i))) };
        if x != y || x == 0 {
            return i32::from(x) - i32::from(y);
        }
    }
    0
}

/// strtok's position (per process here; per thread in the CRT).
static mut STRTOK: *mut u8 = core::ptr::null_mut();

/// strtok(s, delimiters): the next token; s null continues the last.
#[unsafe(no_mangle)]
pub extern "C" fn strtok(s: *mut u8, delimiters: *const u8) -> *mut u8 {
    // SAFETY: the caller's NUL-terminated, writable string and delimiters.
    unsafe {
        let delims = crate::c_str(delimiters);
        let mut p = if s.is_null() { *(&raw const STRTOK) } else { s };
        if p.is_null() {
            return p;
        }
        while *p != 0 && delims.contains(&*p) {
            p = p.add(1);
        }
        if *p == 0 {
            *(&raw mut STRTOK) = core::ptr::null_mut();
            return core::ptr::null_mut();
        }
        let token = p;
        while *p != 0 && !delims.contains(&*p) {
            p = p.add(1);
        }
        if *p == 0 {
            *(&raw mut STRTOK) = core::ptr::null_mut();
        } else {
            *p = 0;
            *(&raw mut STRTOK) = p.add(1);
        }
        token
    }
}

/// bsearch(key, base, count, size, compare).
#[unsafe(no_mangle)]
pub extern "C" fn bsearch(
    key: *const u8,
    base: *const u8,
    count: usize,
    size: usize,
    compare: extern "C" fn(*const u8, *const u8) -> i32,
) -> *mut u8 {
    let (mut lo, mut hi) = (0usize, count);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        // SAFETY: inside the caller's array of `count` elements.
        let item = unsafe { base.add(mid * size) };
        match compare(key, item) {
            0 => return item.cast_mut(),
            c if c < 0 => hi = mid,
            _ => lo = mid + 1,
        }
    }
    core::ptr::null_mut()
}

/// atof(s): leading white space, then a decimal number (0.0 if none).
#[unsafe(no_mangle)]
pub extern "C" fn atof(s: *const u8) -> f64 {
    // SAFETY: the caller's NUL-terminated string.
    let text = unsafe { crate::c_str(s) };
    let start =
        text.iter().position(|&c| !matches!(c, b' ' | b'\t' | b'\n' | b'\r')).unwrap_or(text.len());
    crate::scan::float(&text[start..], usize::MAX).map_or(0.0, |(v, _)| v)
}

/// mbstowcs(dst, src, n): UTF-8 (the ANSI code page) to UTF-16; with a
/// null `dst`, the length needed.
#[unsafe(no_mangle)]
pub extern "C" fn mbstowcs(dst: *mut u16, src: *const u8, n: usize) -> usize {
    // SAFETY: the caller's NUL-terminated string.
    let Ok(text) = core::str::from_utf8(unsafe { crate::c_str(src) }) else { return usize::MAX };
    let mut count = 0;
    for c in text.encode_utf16() {
        if !dst.is_null() {
            if count >= n {
                return count;
            }
            // SAFETY: count < n characters of the caller's buffer.
            unsafe { *dst.add(count) = c };
        }
        count += 1;
    }
    if !dst.is_null() && count < n {
        // SAFETY: as above.
        unsafe { *dst.add(count) = 0 };
    }
    count
}
