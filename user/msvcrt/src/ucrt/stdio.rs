//! The UCRT's stdio: the `__stdio_common_*` functions that its headers'
//! inline printf and scanf call, with their option flags
//! (corecrt_stdio_config.h), the stream functions msvcrt lacks, and
//! `__acrt_iob_func`.
//!
//! Return values by option (learn.microsoft.com's `_snprintf`,
//! `snprintf`, `_vsnprintf_s` pages; Wine's ucrtbase tests pin the rest):
//! with _CRT_INTERNAL_PRINTF_LEGACY_VSPRINTF_NULL_TERMINATION (1) a
//! result that does not fit is -1 and one that just fits is not
//! terminated (msvcrt's `_vsnprintf`); with
//! _CRT_INTERNAL_PRINTF_STANDARD_SNPRINTF_BEHAVIOR (2) the length the
//! whole output needs, the buffer terminated (C99 snprintf); with neither,
//! -2 when it does not fit, terminated at the last character (-1 for a
//! zero-length buffer). A NULL buffer measures.

use super::{EINVAL, ERANGE, TRUNCATE, invalid_parameter};
use crate::scan::{ScanStyle, scan};
use crate::stdio::{Out, Style, format, with_utf8_format};
use crate::va::VaList;
use core::ptr::null_mut;

const LEGACY_VSPRINTF_NULL_TERMINATION: u64 = 1;
const STANDARD_SNPRINTF_BEHAVIOR: u64 = 2;
const LEGACY_WIDE_SPECIFIERS: u64 = 4;
const LEGACY_MSVCRT_COMPATIBILITY: u64 = 8;
const LEGACY_THREE_DIGIT_EXPONENTS: u64 = 16;
/// The flags this runtime knows (`_CRT_INTERNAL_PRINTF_STANDARD_ROUNDING`,
/// 32, and the rest are recorded when asked for).
const KNOWN_PRINTF: u64 = 0x1f;

const SCANF_SECURECRT: u64 = 1;
const SCANF_LEGACY_WIDE_SPECIFIERS: u64 = 2;
const SCANF_LEGACY_MSVCRT_COMPATIBILITY: u64 = 4;

/// The formatting style `options` ask for, in a narrow or wide function.
fn style(options: u64, wide: bool) -> Style {
    if options & !KNOWN_PRINTF != 0 {
        crate::missing(b"ucrtbase.dll!__stdio_common_vsprintf(options)", options);
    }
    if options & LEGACY_MSVCRT_COMPATIBILITY != 0 {
        // `1.#INF` and the `%F`/`%N` length modifiers: not yet.
        crate::missing(b"ucrtbase.dll!__stdio_common_vsprintf(msvcrt compatibility)", options);
    }
    let exponent_digits = if options & LEGACY_THREE_DIGIT_EXPONENTS != 0 { 3 } else { 2 };
    Style {
        wide,
        legacy_wide: options & LEGACY_WIDE_SPECIFIERS != 0,
        float: crate::float::Style { exact: true, exponent_digits },
        ucrt: true,
    }
}

/// The result of formatting `total` characters into a buffer of `len`
/// (`terminate(at)` writes a NUL at `at`), by `options` (module comment).
fn finish(
    options: u64,
    null_buffer: bool,
    len: usize,
    total: usize,
    terminate: impl Fn(usize),
) -> i32 {
    if null_buffer {
        return total as i32;
    }
    if options & LEGACY_VSPRINTF_NULL_TERMINATION != 0 {
        if total < len {
            terminate(total);
        }
        return if total > len { -1 } else { total as i32 };
    }
    if total >= len {
        if len > 0 {
            terminate(len - 1);
        }
        if options & STANDARD_SNPRINTF_BEHAVIOR != 0 {
            return total as i32;
        }
        return if len > 0 { -2 } else { -1 };
    }
    terminate(total);
    total as i32
}

/// `__stdio_common_vsprintf(options, buffer, count, format, locale, args)`.
#[unsafe(no_mangle)]
pub extern "C" fn __stdio_common_vsprintf(
    options: u64,
    buf: *mut u8,
    len: usize,
    fmt: *const u8,
    _locale: usize,
    args: VaList,
) -> i32 {
    if fmt.is_null() || (len > 0 && buf.is_null()) {
        invalid_parameter();
        crate::wide::set_errno(EINVAL);
        return -1;
    }
    let mut out = Out::bytes(buf, if buf.is_null() { 0 } else { len });
    // SAFETY: the caller's NUL-terminated format.
    format(&mut out, unsafe { crate::c_str(fmt) }, args, style(options, false));
    finish(options, buf.is_null(), len, out.total, |at| {
        // SAFETY: inside the caller's buffer.
        unsafe { *buf.add(at) = 0 }
    })
}

/// `__stdio_common_vswprintf`: the same for UTF-16 (counts in characters).
#[unsafe(no_mangle)]
pub extern "C" fn __stdio_common_vswprintf(
    options: u64,
    buf: *mut u16,
    len: usize,
    fmt: *const u16,
    _locale: usize,
    args: VaList,
) -> i32 {
    if fmt.is_null() || (len > 0 && buf.is_null()) {
        invalid_parameter();
        crate::wide::set_errno(EINVAL);
        return -1;
    }
    with_utf8_format(fmt, |f| {
        let mut out = Out::wide(buf, if buf.is_null() { 0 } else { len });
        format(&mut out, f, args, style(options, true));
        finish(options, buf.is_null(), len, out.total, |at| {
            // SAFETY: inside the caller's buffer.
            unsafe { *buf.add(at) = 0 }
        })
    })
}

/// The `_s` forms' room: at most `count` characters (`_TRUNCATE`: what
/// fits) and the terminator, in `size`.
fn secure_room(size: usize, count: usize) -> usize {
    if count == TRUNCATE || size < count.saturating_add(1) { size } else { count + 1 }
}

/// The `_s` forms' result for `total` characters formatted into `room`
/// (`secure_room`): the length; or, when it did not fit, -1 with what
/// fitted terminated (`_TRUNCATE`, or `count` characters asked for that
/// the buffer holds), else (`strict`: the sprintf_s forms, or `count`
/// beyond the buffer) the buffer emptied, ERANGE and the invalid-parameter
/// handler.
fn secure_finish(
    size: usize,
    count: usize,
    room: usize,
    total: usize,
    strict: bool,
    write_nul: impl Fn(usize),
) -> i32 {
    if total < room {
        write_nul(total);
        return total as i32;
    }
    if strict || (count != TRUNCATE && count >= size) {
        write_nul(0);
        invalid_parameter();
        crate::wide::set_errno(ERANGE);
        return -1;
    }
    write_nul(room - 1);
    -1
}

/// The narrow `_s` printf (module comment), `strict` for sprintf_s.
fn snprintf_s(
    options: u64,
    buf: *mut u8,
    size: usize,
    count: usize,
    fmt: *const u8,
    args: VaList,
    strict: bool,
) -> i32 {
    if fmt.is_null() || buf.is_null() || size == 0 {
        invalid_parameter();
        crate::wide::set_errno(EINVAL);
        return -1;
    }
    let room = secure_room(size, count);
    let mut out = Out::bytes(buf, room);
    // SAFETY: the caller's NUL-terminated format.
    format(&mut out, unsafe { crate::c_str(fmt) }, args, style(options, false));
    // SAFETY: inside the caller's buffer of `size`.
    secure_finish(size, count, room, out.total, strict, |at| unsafe { *buf.add(at) = 0 })
}

/// The wide `_s` printf.
fn snwprintf_s(
    options: u64,
    buf: *mut u16,
    size: usize,
    count: usize,
    fmt: *const u16,
    args: VaList,
    strict: bool,
) -> i32 {
    if fmt.is_null() || buf.is_null() || size == 0 {
        invalid_parameter();
        crate::wide::set_errno(EINVAL);
        return -1;
    }
    let room = secure_room(size, count);
    with_utf8_format(fmt, |f| {
        let mut out = Out::wide(buf, room);
        format(&mut out, f, args, style(options, true));
        // SAFETY: inside the caller's buffer of `size`.
        secure_finish(size, count, room, out.total, strict, |at| unsafe { *buf.add(at) = 0 })
    })
}

/// `__stdio_common_vsnprintf_s(options, buffer, size, count, format, locale, args)`.
#[unsafe(no_mangle)]
pub extern "C" fn __stdio_common_vsnprintf_s(
    options: u64,
    buf: *mut u8,
    size: usize,
    count: usize,
    fmt: *const u8,
    _locale: usize,
    args: VaList,
) -> i32 {
    snprintf_s(options, buf, size, count, fmt, args, false)
}

/// `__stdio_common_vsprintf_s(options, buffer, size, format, locale, args)`:
/// the whole output or an invalid parameter.
#[unsafe(no_mangle)]
pub extern "C" fn __stdio_common_vsprintf_s(
    options: u64,
    buf: *mut u8,
    size: usize,
    fmt: *const u8,
    _locale: usize,
    args: VaList,
) -> i32 {
    snprintf_s(options, buf, size, TRUNCATE, fmt, args, true)
}

/// `__stdio_common_vsnwprintf_s`: `__stdio_common_vsnprintf_s` for UTF-16.
#[unsafe(no_mangle)]
pub extern "C" fn __stdio_common_vsnwprintf_s(
    options: u64,
    buf: *mut u16,
    size: usize,
    count: usize,
    fmt: *const u16,
    _locale: usize,
    args: VaList,
) -> i32 {
    snwprintf_s(options, buf, size, count, fmt, args, false)
}

/// `__stdio_common_vswprintf_s`.
#[unsafe(no_mangle)]
pub extern "C" fn __stdio_common_vswprintf_s(
    options: u64,
    buf: *mut u16,
    size: usize,
    fmt: *const u16,
    _locale: usize,
    args: VaList,
) -> i32 {
    snwprintf_s(options, buf, size, TRUNCATE, fmt, args, true)
}

/// `__stdio_common_vfprintf(options, stream, format, locale, args)`.
#[unsafe(no_mangle)]
pub extern "C" fn __stdio_common_vfprintf(
    options: u64,
    file: *mut u8,
    fmt: *const u8,
    _locale: usize,
    args: VaList,
) -> i32 {
    if fmt.is_null() {
        invalid_parameter();
        return -1;
    }
    let Some(handle) = crate::stdio::stream_handle(file) else {
        invalid_parameter();
        return -1;
    };
    let mut out = Out::handle(handle);
    // SAFETY: the caller's NUL-terminated format.
    format(&mut out, unsafe { crate::c_str(fmt) }, args, style(options, false));
    out.flush();
    out.total as i32
}

/// `__stdio_common_vfprintf_s`: the same (the checks are the format's).
#[unsafe(no_mangle)]
pub extern "C" fn __stdio_common_vfprintf_s(
    options: u64,
    file: *mut u8,
    fmt: *const u8,
    locale: usize,
    args: VaList,
) -> i32 {
    __stdio_common_vfprintf(options, file, fmt, locale, args)
}

/// `__stdio_common_vfwprintf(options, stream, format, locale, args)`: the
/// output in the ANSI code page (UTF-8 here), as for a stream not in a
/// Unicode mode; the count of wide characters.
#[unsafe(no_mangle)]
pub extern "C" fn __stdio_common_vfwprintf(
    options: u64,
    file: *mut u8,
    fmt: *const u16,
    _locale: usize,
    args: VaList,
) -> i32 {
    let Some(handle) = crate::stdio::stream_handle(file) else {
        invalid_parameter();
        return -1;
    };
    with_utf8_format(fmt, |f| {
        // Count UTF-16 units: format once into a wide counter, then again
        // to the handle (the arguments are read twice: a va_list copy).
        let mut count = Out::wide(null_mut(), 0);
        format(&mut count, f, args, style(options, true));
        let mut out = Out::handle(handle);
        format(&mut out, f, args, style(options, true));
        out.flush();
        count.total as i32
    })
}

/// `__stdio_common_vsscanf(options, input, count, format, locale, args)`:
/// `count` bytes of input at most (`(size_t)-1`: up to its NUL).
#[unsafe(no_mangle)]
pub extern "C" fn __stdio_common_vsscanf(
    options: u64,
    input: *const u8,
    count: usize,
    fmt: *const u8,
    _locale: usize,
    args: VaList,
) -> i32 {
    if input.is_null() || fmt.is_null() {
        invalid_parameter();
        crate::wide::set_errno(EINVAL);
        return -1;
    }
    if options & SCANF_LEGACY_MSVCRT_COMPATIBILITY != 0 {
        crate::missing(b"ucrtbase.dll!__stdio_common_vsscanf(msvcrt compatibility)", options);
    }
    // SAFETY: the caller's NUL-terminated strings.
    let (s, f) = unsafe { (crate::c_str(input), crate::c_str(fmt)) };
    let s = &s[..s.len().min(count)];
    scan(s, f, args, ScanStyle { secure: options & SCANF_SECURECRT != 0, wide: false })
}

/// `__stdio_common_vswscanf`: the input and format as UTF-8; `%ls`
/// stores UTF-16, and so does `%s` with
/// _CRT_INTERNAL_SCANF_LEGACY_WIDE_SPECIFIERS (the headers' default).
#[unsafe(no_mangle)]
pub extern "C" fn __stdio_common_vswscanf(
    options: u64,
    input: *const u16,
    count: usize,
    fmt: *const u16,
    _locale: usize,
    args: VaList,
) -> i32 {
    if input.is_null() || fmt.is_null() {
        invalid_parameter();
        crate::wide::set_errno(EINVAL);
        return -1;
    }
    // SAFETY: the caller's NUL-terminated input.
    let units = unsafe { core::slice::from_raw_parts(input, crate::wide::wlen(input)) };
    let units = &units[..units.len().min(count)];
    let mut text = [0u8; 2048];
    let mut n = 0;
    for c in char::decode_utf16(units.iter().copied()) {
        let mut b = [0u8; 4];
        let e = c.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut b);
        if n + e.len() > text.len() {
            crate::missing(b"ucrtbase.dll!__stdio_common_vswscanf(long input)", units.len() as u64);
            break;
        }
        text[n..n + e.len()].copy_from_slice(e.as_bytes());
        n += e.len();
    }
    with_utf8_format(fmt, |f| {
        let style = ScanStyle {
            secure: options & SCANF_SECURECRT != 0,
            wide: options & SCANF_LEGACY_WIDE_SPECIFIERS != 0,
        };
        scan(&text[..n], f, args, style)
    })
}

// ---- Streams ----------------------------------------------------------------

/// `__acrt_iob_func(i)`: stdin, stdout, stderr (0, 1, 2).
#[unsafe(no_mangle)]
pub extern "C" fn __acrt_iob_func(index: u32) -> *mut u8 {
    // SAFETY: inside _iob (its first three are the console streams).
    unsafe { crate::stdio::__iob_func().add(index.min(2) as usize * crate::stdio::FILE_SIZE) }
}

/// `_fseeki64(stream, offset, origin)`.
#[unsafe(no_mangle)]
pub extern "C" fn _fseeki64(file: *mut u8, offset: i64, origin: i32) -> i32 {
    crate::stdio::seek(file, offset, origin)
}

/// `_ftelli64(stream)`.
#[unsafe(no_mangle)]
pub extern "C" fn _ftelli64(file: *mut u8) -> i64 {
    crate::stdio::tell(file)
}

/// `_fseeki64_nolock`, `_ftelli64_nolock`, `_fseek_nolock`, `_ftell_nolock`:
/// streams are not locked here.
#[unsafe(no_mangle)]
pub extern "C" fn _fseeki64_nolock(file: *mut u8, offset: i64, origin: i32) -> i32 {
    crate::stdio::seek(file, offset, origin)
}

#[unsafe(no_mangle)]
pub extern "C" fn _ftelli64_nolock(file: *mut u8) -> i64 {
    crate::stdio::tell(file)
}

/// `_fsopen(path, mode, share)`: fopen (gala's files are opened sharing
/// read and write, which every share flag but _SH_DENYRW allows).
#[unsafe(no_mangle)]
pub extern "C" fn _fsopen(path: *const u8, mode: *const u8, _share: i32) -> *mut u8 {
    crate::stdio::fopen(path, mode)
}

/// `_wfsopen(path, mode, share)`.
#[unsafe(no_mangle)]
pub extern "C" fn _wfsopen(path: *const u16, mode: *const u16, _share: i32) -> *mut u8 {
    crate::stdio::_wfopen(path, mode)
}

/// The `fopen_s` family's result: 0 and the stream, or errno and NULL.
fn open_s(out: *mut *mut u8, open: impl FnOnce() -> *mut u8) -> i32 {
    if out.is_null() {
        invalid_parameter();
        return EINVAL;
    }
    let f = open();
    // SAFETY: the caller's FILE* slot.
    unsafe { *out = f };
    if f.is_null() {
        // SAFETY: our errno.
        let e = unsafe { *crate::stdio::_errno() };
        return if e == 0 { EINVAL } else { e };
    }
    0
}

/// `fopen_s(&stream, path, mode)`.
#[unsafe(no_mangle)]
pub extern "C" fn fopen_s(out: *mut *mut u8, path: *const u8, mode: *const u8) -> i32 {
    if path.is_null() || mode.is_null() {
        invalid_parameter();
        return EINVAL;
    }
    open_s(out, || crate::stdio::fopen(path, mode))
}

/// `_wfopen_s(&stream, path, mode)`.
#[unsafe(no_mangle)]
pub extern "C" fn _wfopen_s(out: *mut *mut u8, path: *const u16, mode: *const u16) -> i32 {
    if path.is_null() || mode.is_null() {
        invalid_parameter();
        return EINVAL;
    }
    open_s(out, || crate::stdio::_wfopen(path, mode))
}

/// `freopen_s(&stream, path, mode, old)`.
#[unsafe(no_mangle)]
pub extern "C" fn freopen_s(
    out: *mut *mut u8,
    path: *const u8,
    mode: *const u8,
    old: *mut u8,
) -> i32 {
    open_s(out, || crate::stdio::freopen(path, mode, old))
}

/// `_wfreopen_s(&stream, path, mode, old)`.
#[unsafe(no_mangle)]
pub extern "C" fn _wfreopen_s(
    out: *mut *mut u8,
    path: *const u16,
    mode: *const u16,
    old: *mut u8,
) -> i32 {
    open_s(out, || crate::wide::_wfreopen(path, mode, old))
}

/// `_lock_file`/`_unlock_file`: gala's streams write each call's bytes
/// with one WriteFile and keep no shared buffer, so there is nothing to
/// lock yet.
#[unsafe(no_mangle)]
pub extern "C" fn _lock_file(_file: *mut u8) {}

#[unsafe(no_mangle)]
pub extern "C" fn _unlock_file(_file: *mut u8) {}

/// `_get_stream_buffer_pointers(stream, &base, &ptr, &count)`: the
/// addresses of the FILE's `_base`, `_ptr` and `_cnt` (the inline
/// getc/putc of the UCRT's headers would use them; gala's `_cnt` stays 0,
/// so they always call in).
#[unsafe(no_mangle)]
pub extern "C" fn _get_stream_buffer_pointers(
    file: *mut u8,
    base: *mut *mut *mut u8,
    ptr: *mut *mut *mut u8,
    count: *mut *mut i32,
) -> i32 {
    if file.is_null() {
        invalid_parameter();
        return EINVAL;
    }
    let f = file.cast::<crate::stdio::File>();
    // SAFETY: the caller's out-pointers (each may be NULL); a FILE.
    unsafe {
        if !base.is_null() {
            *base = (&raw mut (*f).base).cast();
        }
        if !ptr.is_null() {
            *ptr = (&raw mut (*f).ptr).cast();
        }
        if !count.is_null() {
            *count = &raw mut (*f).cnt;
        }
    }
    0
}

/// `_fflush_nolock`, `_fclose_nolock`, `_fread_nolock`, `_fwrite_nolock`,
/// `_getc_nolock`'s function forms: as their locking forms.
#[unsafe(no_mangle)]
pub extern "C" fn _fread_nolock(buf: *mut u8, size: usize, count: usize, file: *mut u8) -> usize {
    crate::stdio::fread(buf, size, count, file)
}

#[unsafe(no_mangle)]
pub extern "C" fn _fwrite_nolock(
    buf: *const u8,
    size: usize,
    count: usize,
    file: *mut u8,
) -> usize {
    crate::stdio::fwrite(buf, size, count, file)
}

/// `_sopen_s(&fd, path, flags, share, mode)`: 0 and the descriptor, or
/// errno and -1.
#[unsafe(no_mangle)]
pub extern "C" fn _sopen_s(
    fd: *mut i32,
    path: *const u8,
    flags: i32,
    _share: i32,
    mode: i32,
) -> i32 {
    if fd.is_null() || path.is_null() {
        invalid_parameter();
        return EINVAL;
    }
    let r = crate::stdio::_open(path, flags, mode);
    // SAFETY: the caller's int.
    unsafe { *fd = r };
    if r < 0 {
        // SAFETY: our errno.
        return unsafe { *crate::stdio::_errno() };
    }
    0
}

/// `_wsopen_s(&fd, path, flags, share, mode)`.
#[unsafe(no_mangle)]
pub extern "C" fn _wsopen_s(
    fd: *mut i32,
    path: *const u16,
    flags: i32,
    share: i32,
    mode: i32,
) -> i32 {
    let mut buf = [0u8; 4 * crate::wide::PATH];
    match crate::wide::utf8(path, &mut buf) {
        Some(p) => _sopen_s(fd, p, flags, share, mode),
        None => {
            invalid_parameter();
            EINVAL
        }
    }
}

/// `_chsize_s(fd, size)`: the file's length set to `size`.
#[unsafe(no_mangle)]
pub extern "C" fn _chsize_s(fd: i32, size: i64) -> i32 {
    if size < 0 {
        invalid_parameter();
        return EINVAL;
    }
    if crate::stdio::set_length(fd, size) { 0 } else { unsafe { *crate::stdio::_errno() } }
}

/// `_chsize(fd, size)`: 0 or -1.
#[unsafe(no_mangle)]
pub extern "C" fn _chsize(fd: i32, size: i32) -> i32 {
    if _chsize_s(fd, i64::from(size)) == 0 { 0 } else { -1 }
}

/// `_set_printf_count_output(enable)`: %n stays disabled; the previous
/// setting (0).
#[unsafe(no_mangle)]
pub extern "C" fn _set_printf_count_output(_enable: i32) -> i32 {
    0
}

/// `_get_printf_count_output()`.
#[unsafe(no_mangle)]
pub extern "C" fn _get_printf_count_output() -> i32 {
    0
}

// ---- Finding files: the `64i32` forms ----------------------------------------

/// `struct _finddata64i32_t`: `__finddata64_t` with a 32-bit size.
#[repr(C)]
pub struct FindData64i32 {
    attrib: u32,
    time_create: i64,
    time_access: i64,
    time_write: i64,
    size: u32,
    name: [u8; 260],
}

/// `struct _wfinddata64i32_t`.
#[repr(C)]
pub struct WFindData64i32 {
    attrib: u32,
    time_create: i64,
    time_access: i64,
    time_write: i64,
    size: u32,
    name: [u16; 260],
}

/// A 64-bit find result in the `64i32` form (the size truncated, as the
/// UCRT does for files of 4 GiB and more).
fn narrow_size(d: &crate::posix::FindData64, out: *mut FindData64i32) {
    // SAFETY: the caller's structure.
    unsafe {
        (*out).attrib = d.attrib;
        (*out).time_create = d.time_create;
        (*out).time_access = d.time_access;
        (*out).time_write = d.time_write;
        (*out).size = d.size as u32;
        (*out).name = d.name;
    }
}

fn wide_narrow_size(d: &crate::wide_more::WFindData64, out: *mut WFindData64i32) {
    // SAFETY: the caller's structure.
    unsafe {
        (*out).attrib = d.attrib;
        (*out).time_create = d.time_create;
        (*out).time_access = d.time_access;
        (*out).time_write = d.time_write;
        (*out).size = d.size as u32;
        (*out).name = d.name;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _findfirst64i32(pattern: *const u8, out: *mut FindData64i32) -> isize {
    // SAFETY: zeroed is a valid __finddata64_t.
    let mut d: crate::posix::FindData64 = unsafe { core::mem::zeroed() };
    let h = crate::posix::_findfirst64(pattern, &mut d);
    if h != -1 {
        narrow_size(&d, out);
    }
    h
}

#[unsafe(no_mangle)]
pub extern "C" fn _findnext64i32(h: isize, out: *mut FindData64i32) -> i32 {
    // SAFETY: as above.
    let mut d: crate::posix::FindData64 = unsafe { core::mem::zeroed() };
    let r = crate::posix::_findnext64(h, &mut d);
    if r == 0 {
        narrow_size(&d, out);
    }
    r
}

#[unsafe(no_mangle)]
pub extern "C" fn _wfindfirst64i32(pattern: *const u16, out: *mut WFindData64i32) -> isize {
    // SAFETY: zeroed is a valid _wfinddata64_t.
    let mut d: crate::wide_more::WFindData64 = unsafe { core::mem::zeroed() };
    let h = crate::wide_more::_wfindfirst64(pattern, &mut d);
    if h != -1 {
        wide_narrow_size(&d, out);
    }
    h
}

#[unsafe(no_mangle)]
pub extern "C" fn _wfindnext64i32(h: isize, out: *mut WFindData64i32) -> i32 {
    // SAFETY: as above.
    let mut d: crate::wide_more::WFindData64 = unsafe { core::mem::zeroed() };
    let r = crate::wide_more::_wfindnext64(h, &mut d);
    if r == 0 {
        wide_narrow_size(&d, out);
    }
    r
}
