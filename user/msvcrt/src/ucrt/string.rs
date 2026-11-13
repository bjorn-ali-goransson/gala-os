//! The UCRT's string functions gala's msvcrt lacks: `wmemcpy_s` and
//! `wmemmove_s`, the three-argument `wcstok`, `__wcsncnt` (the `_s` forms
//! both runtimes export are `safe.rs`'s, with the same rules), the
//! 64-bit and wide number conversions, the restartable multibyte
//! functions (the "C" locale's code page is UTF-8 here, as gala's ANSI
//! code page), `strerror_s`, and the byte swaps.

use super::{EINVAL, ERANGE, STRUNCATE, TRUNCATE};
use crate::safe::{bounded_len, fail, move_s, tok};
use crate::wide::{set_errno, wlen};
use core::ptr::null_mut;

#[unsafe(no_mangle)]
pub extern "C" fn wmemcpy_s(dst: *mut u16, size: usize, src: *const u16, count: usize) -> i32 {
    move_s(dst.cast(), size, src.cast(), count, 2)
}

#[unsafe(no_mangle)]
pub extern "C" fn wmemmove_s(dst: *mut u16, size: usize, src: *const u16, count: usize) -> i32 {
    move_s(dst.cast(), size, src.cast(), count, 2)
}

/// `wcstok(s, delim, context)`: the UCRT's three-argument form (a NULL
/// context: the runtime's own).
#[unsafe(no_mangle)]
pub extern "C" fn wcstok(s: *mut u16, delim: *const u16, context: *mut *mut u16) -> *mut u16 {
    static mut CONTEXT: *mut u16 = null_mut();
    let context = if context.is_null() { &raw mut CONTEXT } else { context };
    // SAFETY: the caller's strings; a valid context.
    unsafe { tok(s, delim, context) }
}

#[unsafe(no_mangle)]
pub extern "C" fn __wcsncnt(s: *const u16, max: usize) -> usize {
    bounded_len(s, max)
}

// ---- Numbers to text ------------------------------------------------------------

/// `_ui64toa_s`'s rule over `T`: `v` in `radix` (2 to 36), a minus sign
/// first when `negative` (the signed forms, radix 10); EINVAL or ERANGE
/// (destination emptied) through the handler.
fn to_text<T: From<u8> + Copy>(
    v: u64,
    negative: bool,
    buf: *mut T,
    size: usize,
    radix: i32,
) -> i32 {
    if buf.is_null() || size == 0 {
        return fail(EINVAL);
    }
    if !(2..=36).contains(&radix) {
        // SAFETY: the caller's buffer of `size`.
        unsafe { *buf = T::from(0) };
        return fail(EINVAL);
    }
    let mut digits = [0u8; 66];
    let mut i = digits.len();
    let mut x = v;
    loop {
        i -= 1;
        digits[i] = b"0123456789abcdefghijklmnopqrstuvwxyz"[(x % radix as u64) as usize];
        x /= radix as u64;
        if x == 0 {
            break;
        }
    }
    if negative {
        i -= 1;
        digits[i] = b'-';
    }
    let text = &digits[i..];
    if text.len() + 1 > size {
        // SAFETY: as above.
        unsafe { *buf = T::from(0) };
        return fail(ERANGE);
    }
    for (k, &c) in text.iter().enumerate() {
        // SAFETY: `text.len() + 1 <= size`.
        unsafe { *buf.add(k) = T::from(c) };
    }
    // SAFETY: as above.
    unsafe { *buf.add(text.len()) = T::from(0) };
    0
}

/// A signed value's text: negative only in radix 10 (other radixes show
/// the two's complement of the type's width, `bits`).
fn signed_text<T: From<u8> + Copy>(v: i64, bits: u32, buf: *mut T, size: usize, radix: i32) -> i32 {
    if radix == 10 && v < 0 {
        to_text(v.unsigned_abs(), true, buf, size, radix)
    } else {
        let mask = if bits == 64 { u64::MAX } else { (1u64 << bits) - 1 };
        to_text(v as u64 & mask, false, buf, size, radix)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _itoa_s(v: i32, buf: *mut u8, size: usize, radix: i32) -> i32 {
    signed_text(v.into(), 32, buf, size, radix)
}

#[unsafe(no_mangle)]
pub extern "C" fn _ltoa_s(v: i32, buf: *mut u8, size: usize, radix: i32) -> i32 {
    signed_text(v.into(), 32, buf, size, radix)
}

#[unsafe(no_mangle)]
pub extern "C" fn _ultoa_s(v: u32, buf: *mut u8, size: usize, radix: i32) -> i32 {
    to_text(v.into(), false, buf, size, radix)
}

#[unsafe(no_mangle)]
pub extern "C" fn _i64toa_s(v: i64, buf: *mut u8, size: usize, radix: i32) -> i32 {
    signed_text(v, 64, buf, size, radix)
}

#[unsafe(no_mangle)]
pub extern "C" fn _ui64toa_s(v: u64, buf: *mut u8, size: usize, radix: i32) -> i32 {
    to_text(v, false, buf, size, radix)
}

#[unsafe(no_mangle)]
pub extern "C" fn _itow_s(v: i32, buf: *mut u16, size: usize, radix: i32) -> i32 {
    signed_text(v.into(), 32, buf, size, radix)
}

#[unsafe(no_mangle)]
pub extern "C" fn _ltow_s(v: i32, buf: *mut u16, size: usize, radix: i32) -> i32 {
    signed_text(v.into(), 32, buf, size, radix)
}

#[unsafe(no_mangle)]
pub extern "C" fn _ultow_s(v: u32, buf: *mut u16, size: usize, radix: i32) -> i32 {
    to_text(v.into(), false, buf, size, radix)
}

#[unsafe(no_mangle)]
pub extern "C" fn _i64tow_s(v: i64, buf: *mut u16, size: usize, radix: i32) -> i32 {
    signed_text(v, 64, buf, size, radix)
}

#[unsafe(no_mangle)]
pub extern "C" fn _ui64tow_s(v: u64, buf: *mut u16, size: usize, radix: i32) -> i32 {
    to_text(v, false, buf, size, radix)
}

// ---- Text to numbers -------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn strtoll(s: *const u8, end: *mut *mut u8, base: i32) -> i64 {
    crate::misc::_strtoi64(s, end, base)
}

#[unsafe(no_mangle)]
pub extern "C" fn strtoull(s: *const u8, end: *mut *mut u8, base: i32) -> u64 {
    crate::misc::_strtoui64(s, end, base)
}

#[unsafe(no_mangle)]
pub extern "C" fn strtoimax(s: *const u8, end: *mut *mut u8, base: i32) -> i64 {
    crate::misc::_strtoi64(s, end, base)
}

#[unsafe(no_mangle)]
pub extern "C" fn strtoumax(s: *const u8, end: *mut *mut u8, base: i32) -> u64 {
    crate::misc::_strtoui64(s, end, base)
}

#[unsafe(no_mangle)]
pub extern "C" fn atoll(s: *const u8) -> i64 {
    crate::misc::_strtoi64(s, null_mut(), 10)
}

/// Runs a narrow parser over the wide string `s` (its ASCII; anything
/// else ends the number) and maps its end back: `f(text, &mut end)`.
fn via_ascii<R>(
    s: *const u16,
    end: *mut *mut u16,
    f: impl FnOnce(*const u8, *mut *mut u8) -> R,
) -> R {
    let mut text = [0u8; 512];
    // SAFETY: the caller's NUL-terminated string.
    let n = unsafe { wlen(s) }.min(text.len() - 1);
    for i in 0..n {
        // SAFETY: inside the string.
        let c = unsafe { *s.add(i) };
        text[i] = if c < 0x80 { c as u8 } else { 0x01 };
    }
    let mut stop: *mut u8 = null_mut();
    let r = f(text.as_ptr(), &mut stop);
    if !end.is_null() {
        let used = (stop as usize).saturating_sub(text.as_ptr() as usize);
        // SAFETY: `used` <= n, inside the caller's string.
        unsafe { *end = s.add(used).cast_mut() };
    }
    r
}

#[unsafe(no_mangle)]
pub extern "C" fn _wcstoi64(s: *const u16, end: *mut *mut u16, base: i32) -> i64 {
    via_ascii(s, end, |t, e| crate::misc::_strtoi64(t, e, base))
}

#[unsafe(no_mangle)]
pub extern "C" fn _wcstoui64(s: *const u16, end: *mut *mut u16, base: i32) -> u64 {
    via_ascii(s, end, |t, e| crate::misc::_strtoui64(t, e, base))
}

#[unsafe(no_mangle)]
pub extern "C" fn wcstoll(s: *const u16, end: *mut *mut u16, base: i32) -> i64 {
    _wcstoi64(s, end, base)
}

#[unsafe(no_mangle)]
pub extern "C" fn wcstoull(s: *const u16, end: *mut *mut u16, base: i32) -> u64 {
    _wcstoui64(s, end, base)
}

#[unsafe(no_mangle)]
pub extern "C" fn wcstod(s: *const u16, end: *mut *mut u16) -> f64 {
    via_ascii(s, end, |t, e| crate::misc::strtod(t, e))
}

#[unsafe(no_mangle)]
pub extern "C" fn wcstof(s: *const u16, end: *mut *mut u16) -> f32 {
    wcstod(s, end) as f32
}

#[unsafe(no_mangle)]
pub extern "C" fn strtof(s: *const u8, end: *mut *mut u8) -> f32 {
    crate::misc::strtod(s, end) as f32
}

#[unsafe(no_mangle)]
pub extern "C" fn _wtof(s: *const u16) -> f64 {
    wcstod(s, null_mut())
}

#[unsafe(no_mangle)]
pub extern "C" fn _wtoi64(s: *const u16) -> i64 {
    _wcstoi64(s, null_mut(), 10)
}

#[unsafe(no_mangle)]
pub extern "C" fn _wtoll(s: *const u16) -> i64 {
    _wcstoi64(s, null_mut(), 10)
}

// ---- Multibyte (UTF-8) ----------------------------------------------------------------

/// `mbstate_t`: a partial character's bytes (UTF-8 here).
#[repr(C)]
pub struct MbState {
    bytes: [u8; 4],
    have: u32,
}

/// `mbrtowc(&wc, s, n, state)`: the next character of `s`: 0 for NUL,
/// the bytes it took, (size_t)-2 when incomplete (kept in `state`), or
/// (size_t)-1 with EILSEQ when invalid. Characters beyond the BMP do not
/// fit one wchar_t: U+FFFD (recorded).
#[unsafe(no_mangle)]
pub extern "C" fn mbrtowc(wc: *mut u16, s: *const u8, n: usize, state: *mut MbState) -> usize {
    static mut OWN: MbState = MbState { bytes: [0; 4], have: 0 };
    let st = if state.is_null() { &raw mut OWN } else { state };
    if s.is_null() {
        // SAFETY: a valid state.
        unsafe { (*st).have = 0 };
        return 0;
    }
    // SAFETY: a valid state; the caller's `n` bytes.
    unsafe {
        let st = &mut *st;
        let mut i = 0;
        while i < n {
            st.bytes[st.have as usize] = *s.add(i);
            st.have += 1;
            i += 1;
            let need = match st.bytes[0] {
                0xf0.. => 4,
                0xe0.. => 3,
                0xc0.. => 2,
                0x80.. => {
                    st.have = 0;
                    set_errno(42); // EILSEQ
                    return usize::MAX;
                }
                _ => 1,
            };
            if (st.have as usize) < need {
                continue;
            }
            let have = st.have as usize;
            st.have = 0;
            let Some(c) =
                core::str::from_utf8(&st.bytes[..have]).ok().and_then(|t| t.chars().next())
            else {
                set_errno(42);
                return usize::MAX;
            };
            let unit = if (c as u32) > 0xffff {
                crate::missing(b"ucrtbase.dll!mbrtowc(beyond the BMP)", c as u64);
                0xfffd
            } else {
                c as u16
            };
            if !wc.is_null() {
                *wc = unit;
            }
            return if unit == 0 { 0 } else { i };
        }
        usize::MAX - 1
    }
}

/// `mbrlen(s, n, state)`: `mbrtowc` without storing.
#[unsafe(no_mangle)]
pub extern "C" fn mbrlen(s: *const u8, n: usize, state: *mut MbState) -> usize {
    mbrtowc(null_mut(), s, n, state)
}

/// `wcrtomb(out, wc, state)`: `wc` as UTF-8; its length (a surrogate
/// alone: (size_t)-1, EILSEQ).
#[unsafe(no_mangle)]
pub extern "C" fn wcrtomb(out: *mut u8, wc: u16, _state: *mut MbState) -> usize {
    let Some(c) = char::from_u32(u32::from(wc)) else {
        set_errno(42);
        return usize::MAX;
    };
    let mut b = [0u8; 4];
    let e = c.encode_utf8(&mut b);
    if !out.is_null() {
        // SAFETY: the caller's buffer of MB_CUR_MAX (4 here) bytes.
        unsafe { core::ptr::copy_nonoverlapping(e.as_ptr(), out, e.len()) };
    }
    e.len()
}

/// `wcrtomb_s(&n, out, size, wc, state)`.
#[unsafe(no_mangle)]
pub extern "C" fn wcrtomb_s(
    converted: *mut usize,
    out: *mut u8,
    size: usize,
    wc: u16,
    state: *mut MbState,
) -> i32 {
    let mut b = [0u8; 4];
    let n = wcrtomb(b.as_mut_ptr(), wc, state);
    if n == usize::MAX {
        return 42;
    }
    if !out.is_null() {
        if n > size {
            return fail(ERANGE);
        }
        // SAFETY: room for `n` bytes.
        unsafe { core::ptr::copy_nonoverlapping(b.as_ptr(), out, n) };
    }
    if !converted.is_null() {
        // SAFETY: the caller's size_t.
        unsafe { *converted = n };
    }
    0
}

/// `mbstowcs_s(&converted, dst, size, src, count)`: at most `count`
/// characters (`_TRUNCATE`: what fits) and a NUL; `converted` counts the
/// NUL. ERANGE (emptied) when it does not fit unless truncating
/// (STRUNCATE).
#[unsafe(no_mangle)]
pub extern "C" fn mbstowcs_s(
    converted: *mut usize,
    dst: *mut u16,
    size: usize,
    src: *const u8,
    count: usize,
) -> i32 {
    if src.is_null() || (dst.is_null() && size != 0) {
        return fail(EINVAL);
    }
    // SAFETY: the caller's NUL-terminated string.
    let text = core::str::from_utf8(unsafe { crate::c_str(src) }).unwrap_or("");
    let mut n = 0;
    let mut truncated = false;
    for (i, u) in text.encode_utf16().enumerate() {
        if i >= count {
            break;
        }
        if !dst.is_null() {
            if n + 1 >= size {
                if count == TRUNCATE {
                    truncated = true;
                    break;
                }
                // SAFETY: size > 0.
                unsafe { *dst = 0 };
                return fail(ERANGE);
            }
            // SAFETY: n + 1 < size.
            unsafe { *dst.add(n) = u };
        }
        n += 1;
    }
    if !dst.is_null() {
        // SAFETY: n < size.
        unsafe { *dst.add(n) = 0 };
    }
    if !converted.is_null() {
        // SAFETY: the caller's size_t.
        unsafe { *converted = n + 1 };
    }
    if truncated { STRUNCATE } else { 0 }
}

/// `wcstombs_s(&converted, dst, size, src, count)`: the same in the other
/// direction (`count` in bytes).
#[unsafe(no_mangle)]
pub extern "C" fn wcstombs_s(
    converted: *mut usize,
    dst: *mut u8,
    size: usize,
    src: *const u16,
    count: usize,
) -> i32 {
    if src.is_null() || (dst.is_null() && size != 0) {
        return fail(EINVAL);
    }
    // SAFETY: the caller's NUL-terminated string.
    let units = unsafe { core::slice::from_raw_parts(src, wlen(src)) };
    let mut n = 0;
    let mut truncated = false;
    for c in char::decode_utf16(units.iter().copied()) {
        let mut b = [0u8; 4];
        let e = c.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut b);
        if n + e.len() > count {
            break;
        }
        if !dst.is_null() {
            if n + e.len() + 1 > size {
                if count == TRUNCATE {
                    truncated = true;
                    break;
                }
                // SAFETY: size > 0.
                unsafe { *dst = 0 };
                return fail(ERANGE);
            }
            // SAFETY: room for the character and a NUL.
            unsafe { core::ptr::copy_nonoverlapping(e.as_ptr(), dst.add(n), e.len()) };
        }
        n += e.len();
    }
    if !dst.is_null() {
        // SAFETY: n < size.
        unsafe { *dst.add(n) = 0 };
    }
    if !converted.is_null() {
        // SAFETY: the caller's size_t.
        unsafe { *converted = n + 1 };
    }
    if truncated { STRUNCATE } else { 0 }
}

/// `strerror_s(buf, size, errnum)`: `strerror`'s text, truncated to fit
/// (STRUNCATE).
#[unsafe(no_mangle)]
pub extern "C" fn strerror_s(buf: *mut u8, size: usize, errnum: i32) -> i32 {
    if buf.is_null() || size == 0 {
        return fail(EINVAL);
    }
    // SAFETY: strerror's static NUL-terminated text.
    let text = unsafe { crate::c_str(crate::misc::strerror(errnum)) };
    let n = text.len().min(size - 1);
    // SAFETY: room for `n` bytes and the NUL.
    unsafe {
        core::ptr::copy_nonoverlapping(text.as_ptr(), buf, n);
        *buf.add(n) = 0;
    }
    if n < text.len() { STRUNCATE } else { 0 }
}

// ---- Byte swaps -----------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn _byteswap_ushort(v: u16) -> u16 {
    v.swap_bytes()
}

#[unsafe(no_mangle)]
pub extern "C" fn _byteswap_ulong(v: u32) -> u32 {
    v.swap_bytes()
}

#[unsafe(no_mangle)]
pub extern "C" fn _byteswap_uint64(v: u64) -> u64 {
    v.swap_bytes()
}

// ---- Case, paths, the stack and the heap (App-V's imports) ----------------------

/// `_wcsupr_s`/`_wcslwr_s`'s rule: EINVAL for a NULL string, or (the
/// string emptied) a size of 0 or no terminator within `size`; otherwise
/// the string converted in place (Wine's msvcrt string.c tests).
fn wcs_case_s(s: *mut u16, size: usize, upper: bool) -> i32 {
    if s.is_null() {
        return fail(EINVAL);
    }
    let len = bounded_len(s.cast_const(), size);
    if size == 0 || len == size {
        // SAFETY: the caller's string of at least one element.
        unsafe { *s = 0 };
        return fail(EINVAL);
    }
    for i in 0..len {
        // SAFETY: within the string's `len` characters.
        unsafe {
            let c = s.add(i);
            *c =
                if upper { crate::wide_more::towupper(*c) } else { crate::wide_more::towlower(*c) };
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn _wcsupr_s(s: *mut u16, size: usize) -> i32 {
    wcs_case_s(s, size, true)
}

#[unsafe(no_mangle)]
pub extern "C" fn _wcslwr_s(s: *mut u16, size: usize) -> i32 {
    wcs_case_s(s, size, false)
}

const BACKSLASH: u16 = b'\\' as u16;
const SLASH: u16 = b'/' as u16;
const DOT: u16 = b'.' as u16;
const COLON: u16 = b':' as u16;

/// Writes `c` at `*at` of `path` (`size` elements), keeping the last
/// element for the terminator; false when it does not fit.
fn put(path: *mut u16, size: usize, at: &mut usize, c: u16) -> bool {
    if *at + 1 >= size {
        return false;
    }
    // SAFETY: below `size`.
    unsafe { *path.add(*at) = c };
    *at += 1;
    true
}

/// `_wmakepath_s(path, size, drive, dir, fname, ext)`: the drive's letter
/// and a colon, the directory with a backslash after it unless it ends in
/// one, the name, the extension with a dot before it unless it has one;
/// EINVAL for no buffer or a size of 0; ERANGE when it does not fit, what
/// fit before the terminator's place written and the first element
/// emptied (Wine's msvcrt dir.c test_makepath_s).
#[unsafe(no_mangle)]
pub extern "C" fn _wmakepath_s(
    path: *mut u16,
    size: usize,
    drive: *const u16,
    dir: *const u16,
    fname: *const u16,
    ext: *const u16,
) -> i32 {
    if path.is_null() || size == 0 {
        return fail(EINVAL);
    }
    let text = |p: *const u16| -> &[u16] {
        if p.is_null() {
            return &[];
        }
        // SAFETY: the caller's NUL-terminated string.
        unsafe { core::slice::from_raw_parts(p, wlen(p)) }
    };
    let mut at = 0;
    let fits = (|| {
        let drive = text(drive);
        if let Some(&letter) = drive.first()
            && (!put(path, size, &mut at, letter) || !put(path, size, &mut at, COLON))
        {
            return false;
        }
        let dir = text(dir);
        for &c in dir {
            if !put(path, size, &mut at, c) {
                return false;
            }
        }
        if let Some(&last) = dir.last()
            && last != BACKSLASH
            && last != SLASH
            && !put(path, size, &mut at, BACKSLASH)
        {
            return false;
        }
        for &c in text(fname) {
            if !put(path, size, &mut at, c) {
                return false;
            }
        }
        let ext = text(ext);
        if let Some(&first) = ext.first()
            && first != DOT
            && !put(path, size, &mut at, DOT)
        {
            return false;
        }
        ext.iter().all(|&c| put(path, size, &mut at, c))
    })();
    // SAFETY: `at` < `size`; the first element exists.
    unsafe {
        if !fits {
            *path = 0;
            return fail(ERANGE);
        }
        *path.add(at) = 0;
    }
    0
}

/// `_wsplitpath_s(path, drive, n, dir, n, fname, n, ext, n)`: the drive
/// (`X:`), the directory up to its last separator, the name, the
/// extension from the name's last dot; each output optional (NULL with a
/// size of 0). EINVAL for no path or an output whose pointer and size
/// disagree; ERANGE (every output emptied) when one is too small
/// (documented).
#[unsafe(no_mangle)]
pub extern "C" fn _wsplitpath_s(
    path: *const u16,
    drive: *mut u16,
    drive_size: usize,
    dir: *mut u16,
    dir_size: usize,
    fname: *mut u16,
    fname_size: usize,
    ext: *mut u16,
    ext_size: usize,
) -> i32 {
    let outputs = [(drive, drive_size), (dir, dir_size), (fname, fname_size), (ext, ext_size)];
    let clear = || {
        for &(p, n) in &outputs {
            if !p.is_null() && n > 0 {
                // SAFETY: the caller's buffer of `n` elements.
                unsafe { *p = 0 };
            }
        }
    };
    if path.is_null() || outputs.iter().any(|&(p, n)| p.is_null() != (n == 0)) {
        clear();
        return fail(EINVAL);
    }
    // SAFETY: the caller's NUL-terminated path.
    let p = unsafe { core::slice::from_raw_parts(path, wlen(path)) };
    let drive_end = if p.len() >= 2 && p[1] == COLON { 2 } else { 0 };
    let rest = &p[drive_end..];
    let dir_end =
        drive_end + rest.iter().rposition(|&c| c == BACKSLASH || c == SLASH).map_or(0, |i| i + 1);
    let name = &p[dir_end..];
    let ext_start = dir_end + name.iter().rposition(|&c| c == DOT).unwrap_or(name.len());
    let parts = [&p[..drive_end], &p[drive_end..dir_end], &p[dir_end..ext_start], &p[ext_start..]];
    if outputs.iter().zip(&parts).any(|(&(o, n), part)| !o.is_null() && part.len() >= n) {
        clear();
        return fail(ERANGE);
    }
    for (&(o, _), part) in outputs.iter().zip(&parts) {
        if !o.is_null() {
            // SAFETY: the caller's buffer holds the part and a terminator.
            unsafe {
                core::ptr::copy_nonoverlapping(part.as_ptr(), o, part.len());
                *o.add(part.len()) = 0;
            }
        }
    }
    0
}

/// `_resetstkoflw()`: nonzero (success). A thread's guard page moves down
/// as its stack grows (kernel32's CreateThread lays it out); one used up
/// by a stack overflow is not put back here (recorded).
#[unsafe(no_mangle)]
pub extern "C" fn _resetstkoflw() -> i32 {
    crate::missing(b"ucrtbase.dll!_resetstkoflw(guard page not re-armed)", 0);
    1
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    safe fn GetProcessHeap() -> usize;
}

/// `_get_heap_handle()`: the process heap (the UCRT's heap; gala's
/// malloc keeps its own blocks, so the handle serves heap queries, not
/// HeapFree of malloc's blocks).
#[unsafe(no_mangle)]
pub extern "C" fn _get_heap_handle() -> usize {
    GetProcessHeap()
}
