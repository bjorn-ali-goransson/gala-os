//! The bounds-checked string functions msvcrt.dll and the UCRT both
//! export (the `strcpy_s` family, `memcpy_s`/`memmove_s`, `strtok_s`,
//! `strnlen`/`wcsnlen`; Wine's msvcrt.spec lists them for msvcrt), their
//! rules from learn.microsoft.com: EINVAL for a NULL argument, ERANGE for
//! a buffer too small, with the destination emptied; `_TRUNCATE` copies
//! what fits and returns STRUNCATE. A failure calls the UCRT's
//! invalid-parameter handler in ucrtbase; msvcrt.dll has no such handler
//! to set and returns the code (as Wine's msvcrt tests string.c, run on
//! Windows, expect of it: `wcsncpy_s(NULL, ...)` is EINVAL).

use crate::wide::set_errno;
use core::ptr::null_mut;

pub(crate) const EINVAL: i32 = 22;
pub(crate) const ERANGE: i32 = 34;
pub(crate) const STRUNCATE: i32 = 80;
/// `_TRUNCATE` ((size_t)-1): copy what fits.
pub(crate) const TRUNCATE: usize = usize::MAX;

/// An `_s` function's failure: errno, the handler (the UCRT's), the code.
pub(crate) fn fail(code: i32) -> i32 {
    set_errno(code);
    #[cfg(ucrt)]
    crate::ucrt::invalid_parameter();
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
    if count == 0 && (dst.is_null() && size == 0 || !dst.is_null() && size != 0) {
        // As wcsncpy_s.
        if !dst.is_null() {
            // SAFETY: the caller's buffer of `size` (> 0) elements.
            unsafe { *dst = 0 };
        }
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
    if count == 0 && (dst.is_null() && size == 0 || !dst.is_null() && size != 0) {
        // Nothing to copy: an empty string, whatever the source (Wine's
        // msvcrt string.c test_wcscpy_s, run on Windows).
        if !dst.is_null() {
            // SAFETY: the caller's buffer of `size` (> 0) elements.
            unsafe { *dst = 0 };
        }
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
pub(crate) fn move_s(dst: *mut u8, size: usize, src: *const u8, count: usize, elem: usize) -> i32 {
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

/// The `strtok_s` rule over `T`: skips leading delimiters, ends the token
/// at the next one (replaced by NUL), `*context` after it.
///
/// # Safety
/// The strings are NUL-terminated; `context` is valid.
pub(crate) unsafe fn tok<T: Copy + Default + PartialEq>(
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

/// The length of `s`, at most `max` (elements of `T`).
pub(crate) fn bounded_len<T: Copy + Default + PartialEq>(s: *const T, max: usize) -> usize {
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
