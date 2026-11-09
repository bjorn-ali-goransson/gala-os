//! The UCRT's string functions msvcrt lacks: the bounds-checked `_s`
//! forms (their rules from learn.microsoft.com: EINVAL for a NULL
//! argument, ERANGE for a buffer too small, both through the
//! invalid-parameter handler with the destination emptied; `_TRUNCATE`
//! copies what fits and returns STRUNCATE), the 64-bit and wide number
//! conversions, the restartable multibyte functions (the "C" locale's
//! code page is UTF-8 here, as gala's ANSI code page), `strerror_s`, and
//! the byte swaps.

use super::{EINVAL, ERANGE, STRUNCATE, TRUNCATE, invalid_parameter};
use crate::wide::{set_errno, wlen};
use core::ptr::null_mut;

/// An `_s` function's failure: errno, the handler, the code.
fn fail(code: i32) -> i32 {
    set_errno(code);
    invalid_parameter();
    code
}

/// The `strcpy_s` family over elements of type `T` (u8 or u16): copies
/// at most `count` of `src` (`usize::MAX`: all of it; `TRUNCATE`: what
/// fits) after `keep` elements already in `dst` (the `cat` forms).
///
/// # Safety
/// `dst` holds `size` elements, `src` is NUL-terminated (or holds
/// `count`).
unsafe fn copy_s<T: Copy + Default + PartialEq>(
    dst: *mut T,
    size: usize,
    src: *const T,
    count: usize,
    append: bool,
) -> i32 {
    if dst.is_null() || size == 0 {
        return fail(EINVAL);
    }
    let zero = T::default();
    // SAFETY: per the contract.
    unsafe {
        let mut keep = 0;
        if append {
            while keep < size && *dst.add(keep) != zero {
                keep += 1;
            }
            if keep == size {
                *dst = zero;
                return fail(EINVAL);
            }
        }
        if src.is_null() {
            *dst = zero;
            return fail(EINVAL);
        }
        let mut n = 0;
        let limit = if count == TRUNCATE { usize::MAX } else { count };
        while n < limit && *src.add(n) != zero {
            if keep + n + 1 >= size {
                if count == TRUNCATE {
                    *dst.add(size - 1) = zero;
                    return STRUNCATE;
                }
                *dst = zero;
                return fail(ERANGE);
            }
            *dst.add(keep + n) = *src.add(n);
            n += 1;
        }
        *dst.add(keep + n) = zero;
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn strcpy_s(dst: *mut u8, size: usize, src: *const u8) -> i32 {
    // SAFETY: the caller's buffers, as documented.
    unsafe { copy_s(dst, size, src, usize::MAX - 1, false) }
}

#[unsafe(no_mangle)]
pub extern "C" fn strncpy_s(dst: *mut u8, size: usize, src: *const u8, count: usize) -> i32 {
    if dst.is_null() && size == 0 && count == 0 {
        return 0;
    }
    // SAFETY: the caller's buffers, as documented.
    unsafe { copy_s(dst, size, src, count, false) }
}

#[unsafe(no_mangle)]
pub extern "C" fn strcat_s(dst: *mut u8, size: usize, src: *const u8) -> i32 {
    // SAFETY: the caller's buffers, as documented.
    unsafe { copy_s(dst, size, src, usize::MAX - 1, true) }
}

#[unsafe(no_mangle)]
pub extern "C" fn strncat_s(dst: *mut u8, size: usize, src: *const u8, count: usize) -> i32 {
    // SAFETY: the caller's buffers, as documented.
    unsafe { copy_s(dst, size, src, count, true) }
}

#[unsafe(no_mangle)]
pub extern "C" fn wcscpy_s(dst: *mut u16, size: usize, src: *const u16) -> i32 {
    // SAFETY: the caller's buffers, as documented.
    unsafe { copy_s(dst, size, src, usize::MAX - 1, false) }
}

#[unsafe(no_mangle)]
pub extern "C" fn wcsncpy_s(dst: *mut u16, size: usize, src: *const u16, count: usize) -> i32 {
    if dst.is_null() && size == 0 && count == 0 {
        return 0;
    }
    // SAFETY: the caller's buffers, as documented.
    unsafe { copy_s(dst, size, src, count, false) }
}

#[unsafe(no_mangle)]
pub extern "C" fn wcscat_s(dst: *mut u16, size: usize, src: *const u16) -> i32 {
    // SAFETY: the caller's buffers, as documented.
    unsafe { copy_s(dst, size, src, usize::MAX - 1, true) }
}

#[unsafe(no_mangle)]
pub extern "C" fn wcsncat_s(dst: *mut u16, size: usize, src: *const u16, count: usize) -> i32 {
    // SAFETY: the caller's buffers, as documented.
    unsafe { copy_s(dst, size, src, count, true) }
}

/// `memcpy_s`/`memmove_s` over `elem`-byte elements: `count` of them
/// into `size`; the destination zeroed and ERANGE when it is too small.
fn move_s(dst: *mut u8, size: usize, src: *const u8, count: usize, elem: usize) -> i32 {
    if count == 0 {
        return 0;
    }
    if dst.is_null() {
        return fail(EINVAL);
    }
    if src.is_null() {
        // SAFETY: the caller's buffer of `size` elements.
        unsafe { core::ptr::write_bytes(dst, 0, size * elem) };
        return fail(EINVAL);
    }
    if count > size {
        // SAFETY: as above.
        unsafe { core::ptr::write_bytes(dst, 0, size * elem) };
        return fail(ERANGE);
    }
    // SAFETY: both buffers hold `count` elements (they may overlap).
    unsafe { core::ptr::copy(src, dst, count * elem) };
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn memcpy_s(dst: *mut u8, size: usize, src: *const u8, count: usize) -> i32 {
    move_s(dst, size, src, count, 1)
}

#[unsafe(no_mangle)]
pub extern "C" fn memmove_s(dst: *mut u8, size: usize, src: *const u8, count: usize) -> i32 {
    move_s(dst, size, src, count, 1)
}

#[unsafe(no_mangle)]
pub extern "C" fn wmemcpy_s(dst: *mut u16, size: usize, src: *const u16, count: usize) -> i32 {
    move_s(dst.cast(), size, src.cast(), count, 2)
}

#[unsafe(no_mangle)]
pub extern "C" fn wmemmove_s(dst: *mut u16, size: usize, src: *const u16, count: usize) -> i32 {
    move_s(dst.cast(), size, src.cast(), count, 2)
}

/// The `strtok_s` rule over `T`: skips leading delimiters, ends the token
/// at the next one (replaced by NUL), `*context` after it.
///
/// # Safety
/// The strings are NUL-terminated; `context` is valid.
unsafe fn tok<T: Copy + Default + PartialEq>(
    s: *mut T,
    delim: *const T,
    context: *mut *mut T,
) -> *mut T {
    if delim.is_null() || context.is_null() {
        fail(EINVAL);
        return null_mut();
    }
    let zero = T::default();
    // SAFETY: per the contract.
    unsafe {
        let mut p = if s.is_null() { *context } else { s };
        if p.is_null() {
            fail(EINVAL);
            return null_mut();
        }
        let is_delim = |c: T| {
            let mut d = delim;
            while *d != zero {
                if *d == c {
                    return true;
                }
                d = d.add(1);
            }
            false
        };
        while *p != zero && is_delim(*p) {
            p = p.add(1);
        }
        if *p == zero {
            *context = p;
            return null_mut();
        }
        let start = p;
        while *p != zero && !is_delim(*p) {
            p = p.add(1);
        }
        if *p != zero {
            *p = zero;
            p = p.add(1);
        }
        *context = p;
        start
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn strtok_s(s: *mut u8, delim: *const u8, context: *mut *mut u8) -> *mut u8 {
    // SAFETY: the caller's strings and context.
    unsafe { tok(s, delim, context) }
}

#[unsafe(no_mangle)]
pub extern "C" fn wcstok_s(s: *mut u16, delim: *const u16, context: *mut *mut u16) -> *mut u16 {
    // SAFETY: the caller's strings and context.
    unsafe { tok(s, delim, context) }
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

/// The length of `s`, at most `max` (elements of `T`).
fn bounded_len<T: Copy + Default + PartialEq>(s: *const T, max: usize) -> usize {
    if s.is_null() {
        return 0;
    }
    let mut n = 0;
    // SAFETY: the caller's string, read up to its NUL or `max`.
    while n < max && unsafe { *s.add(n) } != T::default() {
        n += 1;
    }
    n
}

#[unsafe(no_mangle)]
pub extern "C" fn strnlen(s: *const u8, max: usize) -> usize {
    bounded_len(s, max)
}

#[unsafe(no_mangle)]
pub extern "C" fn wcsnlen(s: *const u16, max: usize) -> usize {
    bounded_len(s, max)
}

#[unsafe(no_mangle)]
pub extern "C" fn __strncnt(s: *const u8, max: usize) -> usize {
    bounded_len(s, max)
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
