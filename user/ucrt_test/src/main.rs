//! The Universal C Runtime (ucrtbase.dll, ROADMAP OF): what programs built
//! with Visual C++ 2015 and later import from it, called directly, with
//! the results of Windows' own ucrtbase. One source for two programs:
//! `ucrt_test` (x64) and `wow64_ucrt_test` (x86, built from this file by
//! user/wow64_ucrt_test).
//!
//! Expected values come from Microsoft's documentation of each function
//! (learn.microsoft.com, "C runtime library reference") and from Wine's
//! ucrtbase conformance tests (dlls/ucrtbase/tests, run on Windows),
//! which pin the details the documentation leaves open (the
//! `__stdio_common_*` option flags' return values, the onexit tables).
//!
//! - Start-up: `_initterm` and `_initterm_e` (stops at the first nonzero
//!   result and returns it); onexit tables (`_initialize_onexit_table`
//!   leaves an empty table's three pointers equal, `_register_onexit_function`
//!   appends, NULL entries included, `_execute_onexit_table` runs them last
//!   first, skipping NULLs, and empties the table; -1 for a NULL table);
//!   `_configure_narrow_argv`/`_configure_wide_argv` and `__p___argc`,
//!   `__p___argv`, `__p___wargv` (argv[0] names this program, argv[argc]
//!   is NULL); `_initialize_narrow_environment`/`_wide_` and
//!   `__p__environ`/`__p__wenviron` (`name=value` entries);
//!   `_get_narrow_winmain_command_line` and `_wide_` (the command line
//!   after the program name: quotes toggle, then blanks are skipped);
//!   `_set_app_type`, `__p__commode`, `_set_fmode`/`_get_fmode`,
//!   `_set_new_mode`/`_query_new_mode`, `_seh_filter_exe` (a C++
//!   exception: EXCEPTION_CONTINUE_SEARCH), `__setusermatherr`.
//! - The invalid-parameter handler: `_set_invalid_parameter_handler`
//!   returns the previous one (NULL at first); `_invalid_parameter_noinfo`
//!   and a failing `strcpy_s` call it with NULL expression, function and
//!   file, line 0, and return to the caller.
//!   `_set_thread_local_invalid_parameter_handler` returns the thread's
//!   previous one and leaves the thread's last error as it was.
//! - `__stdio_common_vsprintf` and its wide, `_s` and `snprintf_s`
//!   companions with their option flags: legacy termination (1: -1 when
//!   it does not fit, no terminator when it just fits), standard snprintf
//!   (2: the length needed, terminated), neither (-2 when it does not
//!   fit, terminated); `%e` with two exponent digits (three with flag
//!   16); `inf`/`nan`; `%z`, `%j`, `%T`; exact decimal digits (`%.30f`
//!   of 1/3); `%s` in a wide format is narrow unless flag 4 (legacy wide
//!   specifiers); `_TRUNCATE`.
//! - `__stdio_common_vsscanf` and `vswscanf`: a count limits the input;
//!   the secure flag (1) takes a buffer size after each `%s` pointer.
//! - Streams: `__acrt_iob_func` (descriptors 0, 1, 2), `_wfopen_s`,
//!   `fputs`, `fflush`, `_fseeki64`, `fgets`, `fgetpos`/`fsetpos`,
//!   `ungetc`, `fread`, `_wfsopen`, `_lock_file`/`_unlock_file`,
//!   `setvbuf`, `_get_stream_buffer_pointers` (binary mode: gala's
//!   streams do no CRLF translation yet).
//! - Strings: the `_s` functions (`strcpy_s`, `strncpy_s` with
//!   `_TRUNCATE`: STRUNCATE, `wcscpy_s`, `wcsncpy_s`, `wcsncat_s`,
//!   `wmemcpy_s`, `wcstok_s`), `strnlen`, `wcsnlen`, `__strncnt`, the
//!   number conversions (`_itow_s`, `_ultow_s`, `_i64tow_s`,
//!   `_ui64tow_s`, `strtoll`, `strtoull`, `_wcstoi64`, `_wcstoui64`,
//!   `wcstoull`, `strtof`, `wcstod`, `_wtof`, `_wtoi`, `_wtoi64`, `atol`;
//!   `strtol` overflow: LONG_MAX and ERANGE), `mbrtowc`, `mbrlen`,
//!   `wcrtomb`, `mbstowcs_s`, `wcstombs_s`, `strerror_s`.
//! - Locale: the "C" locale (`setlocale` query), `___mb_cur_max_func` 1,
//!   `__pctype_func`'s classes, `localeconv`'s ".", `_configthreadlocale(0)`
//!   _DISABLE_PER_THREAD_LOCALE (2).
//! - Math: `_dclass`/`_fdclass` (FP_NORMAL -1, FP_ZERO 0, FP_INFINITE 1,
//!   FP_NAN 2, FP_SUBNORMAL -2), `_dsign`, `round` (half away from zero),
//!   `_controlfp_s`; on x86 the `_libm_sse2_*_precise` functions (argument
//!   and result in xmm0).
//! - Time: `_gmtime64_s` (a year after the epoch is 1971-01-01, day 0 of
//!   the year), `_localtime64_s`, `_difftime64`.
//! - Heap and the rest: `_msize` (the size asked for), `_beginthreadex` and
//!   `_endthreadex` (the thread's exit code), `rand` (41 first after
//!   `srand(1)`), `rand_s`, `qsort`, `_byteswap_ulong`/`_ushort`, `_errno`.
//! - It ends with `exit(0)`: the callback of
//!   `_register_thread_local_exe_atexit_callback` runs first (with
//!   DLL_PROCESS_DETACH), then the `_crt_atexit` functions, last
//!   registered first; the last one prints `<name>: atexit ran in order`.
//!
//! Prints `<name>: ok` and exits with 0 (through `exit`), or prints
//! `<name>: check N got X (line L)` and exits with N.

#![no_std]
#![no_main]
// Byte strings with their NUL spelled out, as the C runtime's tests read.
#![allow(clippy::manual_c_str_literals)]

use core::ffi::c_void;
use core::ptr::{null, null_mut};
use core::sync::atomic::{AtomicU32, Ordering};

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "ucrt_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_ucrt_test";

type Handle = usize;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetCommandLineW() -> *const u16;
    fn GetCommandLineA() -> *const u8;
    fn GetTempPathW(len: u32, buf: *mut u16) -> u32;
    fn DeleteFileW(name: *const u16) -> i32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeThread(h: Handle, code: *mut u32) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn SetLastError(error: u32);
    fn GetLastError() -> u32;
}

/// `_onexit_table_t` (corecrt_startup.h).
#[repr(C)]
struct OnexitTable {
    first: usize,
    last: usize,
    end: usize,
}

/// `struct tm` (corecrt_wtime.h): nine ints.
#[repr(C)]
#[derive(Default)]
struct Tm {
    sec: i32,
    min: i32,
    hour: i32,
    mday: i32,
    mon: i32,
    year: i32,
    wday: i32,
    yday: i32,
    isdst: i32,
}

/// The start of `struct lconv` (locale.h): decimal_point first.
#[repr(C)]
struct Lconv {
    decimal_point: *const u8,
}

/// `_Dcomplex` (complex.h).
#[repr(C)]
#[derive(Clone, Copy)]
struct Dcomplex {
    re: f64,
    im: f64,
}

/// `mbstate_t` (corecrt.h): two unsigned longs.
#[repr(C)]
#[derive(Default)]
struct MbState {
    wchar: u32,
    byte: u32,
}

type InvalidParameterHandler = extern "C" fn(*const u16, *const u16, *const u16, u32, usize);
type TlsCallback = extern "system" fn(usize, u32, usize);

// On x86, `import_name_type = "undecorated"` strips one leading underscore
// from a cdecl import's name (rustc treats it as the C decoration), so
// each name is given with one more (`malloc` stays `malloc`).
macro_rules! ucrt_imports {
    ($($(#[$m:meta])* fn $name:ident($($arg:ident: $ty:ty),* $(,)?) $(-> $ret:ty)?;)*) => {
        #[cfg_attr(
            target_arch = "x86",
            link(name = "ucrtbase", kind = "raw-dylib", import_name_type = "undecorated")
        )]
        #[cfg_attr(not(target_arch = "x86"), link(name = "ucrtbase", kind = "raw-dylib"))]
        unsafe extern "C" {
            $(
                $(#[$m])*
                #[cfg_attr(target_arch = "x86", link_name = concat!("_", stringify!($name)))]
                fn $name($($arg: $ty),*) $(-> $ret)?;
            )*
        }
    };
}

ucrt_imports! {
    // Start-up and exit.
    fn _initterm(begin: *const usize, end: *const usize);
    fn _initterm_e(begin: *const usize, end: *const usize) -> i32;
    fn _initialize_onexit_table(table: *mut OnexitTable) -> i32;
    fn _register_onexit_function(table: *mut OnexitTable, f: usize) -> i32;
    fn _execute_onexit_table(table: *mut OnexitTable) -> i32;
    fn _crt_atexit(f: extern "C" fn()) -> i32;
    fn _register_thread_local_exe_atexit_callback(f: TlsCallback);
    fn _configure_narrow_argv(mode: i32) -> i32;
    fn _configure_wide_argv(mode: i32) -> i32;
    fn __p___argc() -> *mut i32;
    fn __p___argv() -> *mut *mut *mut u8;
    fn __p___wargv() -> *mut *mut *mut u16;
    fn _initialize_narrow_environment() -> i32;
    fn _initialize_wide_environment() -> i32;
    fn __p__environ() -> *mut *mut *mut u8;
    fn __p__wenviron() -> *mut *mut *mut u16;
    fn _get_narrow_winmain_command_line() -> *const u8;
    fn _get_wide_winmain_command_line() -> *const u16;
    fn _set_app_type(kind: i32);
    fn __p__commode() -> *mut i32;
    fn _set_fmode(mode: i32) -> i32;
    fn _get_fmode(mode: *mut i32) -> i32;
    fn _set_new_mode(mode: i32) -> i32;
    fn _query_new_mode() -> i32;
    fn _seh_filter_exe(code: u32, pointers: *mut c_void) -> i32;
    fn __setusermatherr(f: usize);
    fn _set_invalid_parameter_handler(h: Option<InvalidParameterHandler>) -> usize;
    fn _set_thread_local_invalid_parameter_handler(h: Option<InvalidParameterHandler>) -> usize;
    fn _invalid_parameter_noinfo();
    fn _errno() -> *mut i32;
    fn exit(code: i32) -> !;

    // stdio.
    fn __acrt_iob_func(index: u32) -> *mut c_void;
    fn __stdio_common_vsprintf(
        options: u64,
        buf: *mut u8,
        count: usize,
        fmt: *const u8,
        locale: usize,
        args: *const u8,
    ) -> i32;
    fn __stdio_common_vsprintf_s(
        options: u64,
        buf: *mut u8,
        count: usize,
        fmt: *const u8,
        locale: usize,
        args: *const u8,
    ) -> i32;
    fn __stdio_common_vsnprintf_s(
        options: u64,
        buf: *mut u8,
        size: usize,
        count: usize,
        fmt: *const u8,
        locale: usize,
        args: *const u8,
    ) -> i32;
    fn __stdio_common_vswprintf(
        options: u64,
        buf: *mut u16,
        count: usize,
        fmt: *const u16,
        locale: usize,
        args: *const u8,
    ) -> i32;
    fn __stdio_common_vswprintf_s(
        options: u64,
        buf: *mut u16,
        count: usize,
        fmt: *const u16,
        locale: usize,
        args: *const u8,
    ) -> i32;
    fn __stdio_common_vsnwprintf_s(
        options: u64,
        buf: *mut u16,
        size: usize,
        count: usize,
        fmt: *const u16,
        locale: usize,
        args: *const u8,
    ) -> i32;
    fn __stdio_common_vsscanf(
        options: u64,
        input: *const u8,
        count: usize,
        fmt: *const u8,
        locale: usize,
        args: *const u8,
    ) -> i32;
    fn __stdio_common_vswscanf(
        options: u64,
        input: *const u16,
        count: usize,
        fmt: *const u16,
        locale: usize,
        args: *const u8,
    ) -> i32;
    fn _fileno(f: *mut c_void) -> i32;
    fn _wfopen_s(f: *mut *mut c_void, name: *const u16, mode: *const u16) -> i32;
    fn _wfsopen(name: *const u16, mode: *const u16, share: i32) -> *mut c_void;
    fn fputs(s: *const u8, f: *mut c_void) -> i32;
    fn fflush(f: *mut c_void) -> i32;
    fn _fseeki64(f: *mut c_void, offset: i64, origin: i32) -> i32;
    fn fgets(buf: *mut u8, n: i32, f: *mut c_void) -> *mut u8;
    fn fgetpos(f: *mut c_void, pos: *mut i64) -> i32;
    fn fsetpos(f: *mut c_void, pos: *const i64) -> i32;
    fn fgetc(f: *mut c_void) -> i32;
    fn ungetc(c: i32, f: *mut c_void) -> i32;
    fn fread(buf: *mut u8, size: usize, count: usize, f: *mut c_void) -> usize;
    fn fclose(f: *mut c_void) -> i32;
    fn _lock_file(f: *mut c_void);
    fn _unlock_file(f: *mut c_void);
    fn setvbuf(f: *mut c_void, buf: *mut u8, mode: i32, size: usize) -> i32;
    fn _get_stream_buffer_pointers(
        f: *mut c_void,
        base: *mut *mut *mut u8,
        ptr: *mut *mut *mut u8,
        count: *mut *mut i32,
    ) -> i32;

    // Strings.
    fn strcpy_s(dst: *mut u8, size: usize, src: *const u8) -> i32;
    fn strncpy_s(dst: *mut u8, size: usize, src: *const u8, count: usize) -> i32;
    fn wcscpy_s(dst: *mut u16, size: usize, src: *const u16) -> i32;
    fn wcsncpy_s(dst: *mut u16, size: usize, src: *const u16, count: usize) -> i32;
    fn wcsncat_s(dst: *mut u16, size: usize, src: *const u16, count: usize) -> i32;
    fn wmemcpy_s(dst: *mut u16, size: usize, src: *const u16, count: usize) -> i32;
    fn wcstok_s(s: *mut u16, delim: *const u16, context: *mut *mut u16) -> *mut u16;
    fn strnlen(s: *const u8, max: usize) -> usize;
    fn wcsnlen(s: *const u16, max: usize) -> usize;
    fn __strncnt(s: *const u8, max: usize) -> usize;
    fn _itow_s(v: i32, buf: *mut u16, size: usize, radix: i32) -> i32;
    fn _ultow_s(v: u32, buf: *mut u16, size: usize, radix: i32) -> i32;
    fn _i64tow_s(v: i64, buf: *mut u16, size: usize, radix: i32) -> i32;
    fn _ui64tow_s(v: u64, buf: *mut u16, size: usize, radix: i32) -> i32;
    fn strtoll(s: *const u8, end: *mut *const u8, base: i32) -> i64;
    fn strtoull(s: *const u8, end: *mut *const u8, base: i32) -> u64;
    fn strtol(s: *const u8, end: *mut *const u8, base: i32) -> i32;
    fn _wcstoi64(s: *const u16, end: *mut *const u16, base: i32) -> i64;
    fn _wcstoui64(s: *const u16, end: *mut *const u16, base: i32) -> u64;
    fn wcstoull(s: *const u16, end: *mut *const u16, base: i32) -> u64;
    fn strtof(s: *const u8, end: *mut *const u8) -> f32;
    fn wcstod(s: *const u16, end: *mut *const u16) -> f64;
    fn _wtof(s: *const u16) -> f64;
    fn _wtoi(s: *const u16) -> i32;
    fn _wtoi64(s: *const u16) -> i64;
    fn atol(s: *const u8) -> i32;
    fn mbrtowc(wc: *mut u16, s: *const u8, n: usize, state: *mut MbState) -> usize;
    fn mbrlen(s: *const u8, n: usize, state: *mut MbState) -> usize;
    fn wcrtomb(out: *mut u8, wc: u16, state: *mut MbState) -> usize;
    fn mbstowcs_s(
        converted: *mut usize,
        dst: *mut u16,
        size: usize,
        src: *const u8,
        count: usize,
    ) -> i32;
    fn wcstombs_s(
        converted: *mut usize,
        dst: *mut u8,
        size: usize,
        src: *const u16,
        count: usize,
    ) -> i32;
    fn strerror_s(buf: *mut u8, size: usize, errnum: i32) -> i32;
    fn _wcsupr_s(s: *mut u16, size: usize) -> i32;
    fn _wcslwr_s(s: *mut u16, size: usize) -> i32;
    fn _wsplitpath_s(
        path: *const u16,
        drive: *mut u16,
        drive_size: usize,
        dir: *mut u16,
        dir_size: usize,
        fname: *mut u16,
        fname_size: usize,
        ext: *mut u16,
        ext_size: usize,
    ) -> i32;
    fn _wmakepath_s(
        path: *mut u16,
        size: usize,
        drive: *const u16,
        dir: *const u16,
        fname: *const u16,
        ext: *const u16,
    ) -> i32;
    fn _resetstkoflw() -> i32;
    fn _get_heap_handle() -> usize;

    // Locale.
    fn setlocale(category: i32, locale: *const u8) -> *const u8;
    fn ___mb_cur_max_func() -> i32;
    fn __pctype_func() -> *const u16;
    fn localeconv() -> *const Lconv;
    fn _configthreadlocale(mode: i32) -> i32;

    // Math.
    fn _dclass(x: f64) -> i16;
    fn _fdclass(x: f32) -> i16;
    fn _dsign(x: f64) -> i32;
    fn round(x: f64) -> f64;
    fn _controlfp_s(current: *mut u32, new: u32, mask: u32) -> i32;
    fn cabs(z: Dcomplex) -> f64;
    fn cimag(z: Dcomplex) -> f64;
    fn cexp(z: Dcomplex) -> Dcomplex;
    fn _dpcomp(x: f64, y: f64) -> i32;
    fn feclearexcept(excepts: i32) -> i32;
    fn fetestexcept(excepts: i32) -> i32;
    fn btowc(c: i32) -> u16;

    // Time.
    fn _gmtime64_s(tm: *mut Tm, t: *const i64) -> i32;
    fn _localtime64_s(tm: *mut Tm, t: *const i64) -> i32;
    fn _difftime64(a: i64, b: i64) -> f64;

    // Heap, threads, utility.
    fn malloc(size: usize) -> *mut u8;
    fn free(p: *mut u8);
    fn _msize(p: *mut u8) -> usize;
    fn _beginthreadex(
        security: usize,
        stack: u32,
        start: extern "system" fn(*mut c_void) -> u32,
        arg: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn _endthreadex(code: u32) -> !;
    fn srand(seed: u32);
    fn rand() -> i32;
    fn rand_s(v: *mut u32) -> i32;
    fn qsort(
        base: *mut c_void,
        n: usize,
        size: usize,
        cmp: extern "C" fn(*const c_void, *const c_void) -> i32,
    );
    fn _byteswap_ulong(v: u32) -> u32;
    fn _byteswap_ushort(v: u16) -> u16;
}

// Argument and result in xmm0 (not cdecl's stack and st0): called through
// `sse2` below.
#[cfg(target_arch = "x86")]
ucrt_imports! {
    fn _libm_sse2_exp_precise();
    fn _libm_sse2_log_precise();
    fn _libm_sse2_log10_precise();
    fn _libm_sse2_pow_precise();
}

const EINVAL: i32 = 22;
const ERANGE: i32 = 34;
const STRUNCATE: i32 = 80;
const TRUNCATE: usize = usize::MAX;
const ENOENT: i32 = 2;

// ---- Output and checks ---------------------------------------------------

fn print(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid arguments.
    unsafe {
        WriteFile(GetStdHandle(-11i32 as u32), text.as_ptr(), text.len() as u32, &mut written, 0)
    };
}

/// A line built in pieces, written with one WriteFile.
struct Line {
    buf: [u8; 160],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 160], n: 0 }
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn dec(self, v: u64) -> Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&d[i..])
    }

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 18];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..16 {
            d[2 + i] = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
    }

    fn print(self) {
        print(&self.buf[..self.n]);
    }
}

struct Checks(u32);

impl Checks {
    /// A failure prints `<name>: check N got X (line L)`.
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: i64, want: i64) -> Result<(), u32> {
        self.ok(got == want, got as u64)
    }

    /// A NUL-terminated byte string equal to `want`.
    #[track_caller]
    fn str(&mut self, got: *const u8, want: &[u8]) -> Result<(), u32> {
        // SAFETY: a NUL-terminated string from the runtime or our buffer.
        let s = unsafe { cstr(got) };
        let first = s.first().copied().unwrap_or(0);
        self.ok(s == want, (s.len() as u64) << 32 | u64::from(first))
    }

    /// A NUL-terminated UTF-16 string equal to the ASCII `want`.
    #[track_caller]
    fn wstr(&mut self, got: *const u16, want: &[u8]) -> Result<(), u32> {
        // SAFETY: a NUL-terminated string from the runtime or our buffer.
        let s = unsafe { wcstr(got) };
        let same = s.len() == want.len() && s.iter().zip(want).all(|(&a, &b)| a == u16::from(b));
        self.ok(same, (s.len() as u64) << 32 | u64::from(s.first().copied().unwrap_or(0)))
    }
}

/// # Safety
/// `p` is a NUL-terminated string.
unsafe fn cstr<'a>(p: *const u8) -> &'a [u8] {
    if p.is_null() {
        return b"(NULL)";
    }
    let mut n = 0;
    // SAFETY: per the contract.
    unsafe {
        while p.add(n).read_volatile() != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    }
}

/// # Safety
/// `p` is a NUL-terminated UTF-16 string.
unsafe fn wcstr<'a>(p: *const u16) -> &'a [u16] {
    if p.is_null() {
        return &[];
    }
    let mut n = 0;
    // SAFETY: per the contract.
    unsafe {
        while p.add(n).read_volatile() != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    }
}

/// An ASCII string as NUL-terminated UTF-16.
fn w<const N: usize>(s: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (o, b) in out.iter_mut().zip(s.bytes()) {
        *o = u16::from(b);
    }
    out
}

/// A C `va_list` built from Rust values, laid out as the architecture's
/// calling convention passes variadic arguments (x64: one 8-byte slot
/// each; x86: 4-byte words, a double or __int64 taking two).
struct Va {
    words: [usize; 24],
    n: usize,
}

impl Va {
    fn new() -> Self {
        Self { words: [0; 24], n: 0 }
    }

    fn push(mut self, v: usize) -> Self {
        self.words[self.n] = v;
        self.n += 1;
        self
    }

    fn int(self, v: i32) -> Self {
        self.push(v as isize as usize)
    }

    fn ptr<T>(self, p: *const T) -> Self {
        self.push(p as usize)
    }

    /// A pointer the callee writes through.
    fn out<T>(self, p: *mut T) -> Self {
        self.push(p as usize)
    }

    fn size(self, v: usize) -> Self {
        self.push(v)
    }

    fn i64(self, v: i64) -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            self.push(v as usize)
        }
        #[cfg(target_arch = "x86")]
        {
            self.push(v as u32 as usize).push((v as u64 >> 32) as usize)
        }
    }

    fn f64(self, v: f64) -> Self {
        self.i64(v.to_bits() as i64)
    }

    fn list(&self) -> *const u8 {
        self.words.as_ptr().cast()
    }
}

// ---- Start-up ------------------------------------------------------------------

static COUNTER: AtomicU32 = AtomicU32::new(0);

extern "C" fn bump() {
    COUNTER.fetch_add(1, Ordering::Relaxed);
}

extern "C" fn bump_ok() -> i32 {
    COUNTER.fetch_add(1, Ordering::Relaxed);
    0
}

extern "C" fn fail_5() -> i32 {
    5
}

/// Appends its number (1 or 2) to COUNTER's decimal digits.
extern "C" fn onexit_1() -> i32 {
    let v = COUNTER.load(Ordering::Relaxed);
    COUNTER.store(v * 10 + 1, Ordering::Relaxed);
    0
}

extern "C" fn onexit_2() -> i32 {
    let v = COUNTER.load(Ordering::Relaxed);
    COUNTER.store(v * 10 + 2, Ordering::Relaxed);
    0
}

fn startup(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout.
    unsafe {
        let table = [bump as *const () as usize, 0, bump as *const () as usize];
        COUNTER.store(0, Ordering::Relaxed);
        _initterm(table.as_ptr(), table.as_ptr().add(3));
        c.eq(COUNTER.load(Ordering::Relaxed).into(), 2)?;

        let table = [
            bump_ok as *const () as usize,
            0,
            fail_5 as *const () as usize,
            bump_ok as *const () as usize,
        ];
        COUNTER.store(0, Ordering::Relaxed);
        c.eq(_initterm_e(table.as_ptr(), table.as_ptr().add(4)).into(), 5)?;
        c.eq(COUNTER.load(Ordering::Relaxed).into(), 1)?;
        c.eq(_initterm_e(table.as_ptr(), table.as_ptr().add(2)).into(), 0)?;

        // Onexit tables.
        c.eq(_initialize_onexit_table(null_mut()).into(), -1)?;
        let mut t = OnexitTable { first: 0, last: 0, end: 0 };
        c.eq(_initialize_onexit_table(&mut t).into(), 0)?;
        c.ok(t.first == t.last && t.first == t.end, t.last as u64)?;
        let mut u = OnexitTable { first: 0x123, last: 0x456, end: 0x789 };
        c.eq(_initialize_onexit_table(&mut u).into(), 0)?;
        c.ok(u.first == 0x123 && u.last == 0x456 && u.end == 0x789, u.first as u64)?;
        c.eq(_register_onexit_function(null_mut(), onexit_1 as *const () as usize).into(), -1)?;
        c.eq(_register_onexit_function(&mut t, onexit_1 as *const () as usize).into(), 0)?;
        c.eq(_register_onexit_function(&mut t, 0).into(), 0)?;
        c.eq(_register_onexit_function(&mut t, onexit_2 as *const () as usize).into(), 0)?;
        c.ok(t.first != t.end, t.first as u64)?;
        COUNTER.store(0, Ordering::Relaxed);
        c.eq(_execute_onexit_table(&mut t).into(), 0)?;
        c.eq(COUNTER.load(Ordering::Relaxed).into(), 21)?;
        c.ok(t.first == t.end, t.first as u64)?;
        COUNTER.store(0, Ordering::Relaxed);
        c.eq(_execute_onexit_table(&mut t).into(), 0)?;
        c.eq(COUNTER.load(Ordering::Relaxed).into(), 0)?;
        c.eq(_execute_onexit_table(null_mut()).into(), -1)?;

        // The arguments.
        _set_app_type(1);
        c.eq(_configure_narrow_argv(1).into(), 0)?;
        let argc = *__p___argc();
        c.ok(argc >= 1, argc as u64)?;
        let argv = *__p___argv();
        c.ok(!argv.is_null(), 0)?;
        c.ok((*argv.add(argc as usize)).is_null(), 0)?;
        let arg0 = cstr(*argv);
        c.ok(contains_ci(arg0, NAME.as_bytes()), arg0.len() as u64)?;
        c.eq(_configure_wide_argv(1).into(), 0)?;
        let wargv = *__p___wargv();
        c.ok(!wargv.is_null(), 0)?;
        let warg0 = wcstr(*wargv);
        c.ok(
            warg0.len() == arg0.len() && warg0.iter().zip(arg0).all(|(&a, &b)| a == u16::from(b)),
            warg0.len() as u64,
        )?;

        // The environment.
        c.eq(_initialize_narrow_environment().into(), 0)?;
        let env = *__p__environ();
        c.ok(!env.is_null() && !(*env).is_null(), 0)?;
        c.ok(cstr(*env).contains(&b'='), 0)?;
        c.eq(_initialize_wide_environment().into(), 0)?;
        let wenv = *__p__wenviron();
        c.ok(!wenv.is_null() && !(*wenv).is_null(), 0)?;
        c.ok(wcstr(*wenv).contains(&u16::from(b'=')), 0)?;

        // WinMain's command line: after the program name (quotes toggle),
        // then blanks skipped.
        let line = wcstr(GetCommandLineW());
        let (mut i, mut quoted) = (0, false);
        while i < line.len() && (line[i] > 0x20 || (quoted && line[i] != 0)) {
            if line[i] == u16::from(b'"') {
                quoted = !quoted;
            }
            i += 1;
        }
        while i < line.len() && line[i] <= 0x20 {
            i += 1;
        }
        let rest = &line[i..];
        let got = wcstr(_get_wide_winmain_command_line());
        c.ok(got == rest, got.len() as u64)?;
        let narrow = _get_narrow_winmain_command_line();
        let narrow_line = cstr(GetCommandLineA());
        c.ok(cstr(narrow).len() == rest.len(), cstr(narrow).len() as u64)?;
        c.ok(narrow_line.ends_with(cstr(narrow)), 0)?;

        c.ok(!__p__commode().is_null(), 0)?;
        c.eq(_set_fmode(0x8000).into(), 0)?;
        let mut mode = 0;
        c.eq(_get_fmode(&mut mode).into(), 0)?;
        c.eq(mode.into(), 0x8000)?;
        c.eq(_set_fmode(0x4000).into(), 0)?;
        c.eq(_set_new_mode(1).into(), 0)?;
        c.eq(_query_new_mode().into(), 1)?;
        c.eq(_set_new_mode(0).into(), 1)?;
        __setusermatherr(0);
        // A C++ exception: no signal for it, the search goes on.
        let mut record = [0usize; 20];
        record[0] = 0xe06d_7363;
        let mut pointers = [record.as_mut_ptr() as usize, 0];
        c.eq(_seh_filter_exe(0xe06d_7363, pointers.as_mut_ptr().cast()).into(), 0)?;
        *_errno() = 7;
        c.eq((*_errno()).into(), 7)?;
    }
    Ok(())
}

fn contains_ci(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
}

// ---- The invalid-parameter handler ---------------------------------------------

/// Calls of `on_invalid`, and whether every argument was NULL/0.
static INVALID: AtomicU32 = AtomicU32::new(0);
static INVALID_ARGS_NULL: AtomicU32 = AtomicU32::new(1);

extern "C" fn on_invalid(e: *const u16, f: *const u16, file: *const u16, line: u32, arg: usize) {
    INVALID.fetch_add(1, Ordering::Relaxed);
    if !e.is_null() || !f.is_null() || !file.is_null() || line != 0 || arg != 0 {
        INVALID_ARGS_NULL.store(0, Ordering::Relaxed);
    }
}

fn invalid_parameter(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout.
    unsafe {
        c.eq(_set_invalid_parameter_handler(Some(on_invalid)) as i64, 0)?;
        _invalid_parameter_noinfo();
        c.eq(INVALID.load(Ordering::Relaxed).into(), 1)?;
        let mut buf = [0xffu8; 4];
        c.eq(strcpy_s(buf.as_mut_ptr(), 2, b"abc\0".as_ptr()).into(), ERANGE.into())?;
        c.eq(buf[0].into(), 0)?;
        c.eq(INVALID.load(Ordering::Relaxed).into(), 2)?;
        c.eq(INVALID_ARGS_NULL.load(Ordering::Relaxed).into(), 1)?;
        c.eq(strcpy_s(null_mut(), 2, b"abc\0".as_ptr()).into(), EINVAL.into())?;
        c.eq(INVALID.load(Ordering::Relaxed).into(), 3)?;
        // The handler stays installed for the rest of the test (the `_s`
        // functions' failures below call it).
        let previous = _set_invalid_parameter_handler(Some(on_invalid));
        c.eq(previous as i64, on_invalid as *const () as usize as i64)?;
        // The thread's own handler, set and put back around a Win32 call
        // as CPython's _Py_BEGIN/END_SUPPRESS_IPH does: the CRT keeps the
        // thread's last error (os.unlink's failure read it afterwards).
        SetLastError(5);
        let old = _set_thread_local_invalid_parameter_handler(Some(on_invalid));
        c.eq(old as i64, 0)?;
        c.eq(GetLastError().into(), 5)?;
        let mine = _set_thread_local_invalid_parameter_handler(None);
        c.eq(mine as i64, on_invalid as *const () as usize as i64)?;
        c.eq(GetLastError().into(), 5)?;
    }
    Ok(())
}

// ---- printf ---------------------------------------------------------------------

const LEGACY_TERMINATION: u64 = 1;
const STANDARD_SNPRINTF: u64 = 2;
const LEGACY_WIDE: u64 = 4;
const THREE_DIGIT_EXPONENTS: u64 = 16;
const STANDARD_ROUNDING: u64 = 32;

/// `__stdio_common_vsprintf(options, buf, len, fmt, NULL, args)`.
fn sprintf(options: u64, buf: &mut [u8], len: usize, fmt: &[u8], args: &Va) -> i32 {
    // SAFETY: `fmt` is NUL-terminated; `buf` has room for `len`.
    unsafe { __stdio_common_vsprintf(options, buf.as_mut_ptr(), len, fmt.as_ptr(), 0, args.list()) }
}

fn printf(c: &mut Checks) -> Result<(), u32> {
    let none = Va::new();
    let mut buf = [0u8; 100];
    // Termination and the result, by option.
    c.eq(sprintf(0, &mut buf, 8, b"short\0", &none).into(), 5)?;
    c.str(buf.as_ptr(), b"short")?;
    buf = [b'x'; 100];
    c.eq(sprintf(LEGACY_TERMINATION, &mut buf, 8, b"justfits\0", &none).into(), 8)?;
    c.ok(&buf[..9] == b"justfitsx", u64::from(buf[8]))?;
    c.eq(sprintf(LEGACY_TERMINATION, &mut buf, 8, b"muchlonger\0", &none).into(), -1)?;
    c.eq(sprintf(STANDARD_SNPRINTF, &mut buf, 8, b"muchlonger\0", &none).into(), 10)?;
    c.str(buf.as_ptr(), b"muchlon")?;
    buf = [b'x'; 100];
    c.eq(sprintf(0, &mut buf, 8, b"muchlonger\0", &none).into(), -2)?;
    c.str(buf.as_ptr(), b"muchlon")?;
    // SAFETY: a NULL buffer of length 0 measures.
    let measured =
        unsafe { __stdio_common_vsprintf(0, null_mut(), 0, b"abcd\0".as_ptr(), 0, none.list()) };
    c.eq(measured.into(), 4)?;
    c.eq(sprintf(0, &mut buf, 0, b"\0", &none).into(), -1)?;
    c.eq(sprintf(STANDARD_SNPRINTF, &mut buf, 0, b"abcd\0", &none).into(), 4)?;
    c.eq(sprintf(LEGACY_TERMINATION, &mut buf, 0, b"abcd\0", &none).into(), -1)?;

    // Conversions.
    let args = Va::new().int(42).ptr(b"ab\0".as_ptr()).f64(3.5).int(255).int(i32::from(b'z'));
    c.eq(sprintf(0, &mut buf, 100, b"%d-%s-%05.1f-%x-%c\0", &args).into(), 16)?;
    c.str(buf.as_ptr(), b"42-ab-003.5-ff-z")?;
    let args = Va::new().f64(1.23);
    c.eq(sprintf(0, &mut buf, 100, b"%E\0", &args).into(), 12)?;
    c.str(buf.as_ptr(), b"1.230000E+00")?;
    sprintf(THREE_DIGIT_EXPONENTS, &mut buf, 100, b"%E\0", &args);
    c.str(buf.as_ptr(), b"1.230000E+000")?;
    sprintf(0, &mut buf, 100, b"%E\0", &Va::new().f64(1.23e123));
    c.str(buf.as_ptr(), b"1.230000E+123")?;
    sprintf(0, &mut buf, 100, b"%g %g\0", &Va::new().f64(0.00005).f64(1e15));
    c.str(buf.as_ptr(), b"5e-05 1e+15")?;
    let inf = f64::INFINITY;
    let args = Va::new().f64(inf).f64(inf).f64(-inf).f64(inf).f64(inf).f64(inf).f64(inf);
    sprintf(0, &mut buf, 100, b"%f %F %f %e %E %g %G\0", &args);
    c.str(buf.as_ptr(), b"inf INF -inf inf INF inf INF")?;
    sprintf(0, &mut buf, 100, b"%f %F\0", &Va::new().f64(f64::NAN).f64(f64::NAN));
    c.str(buf.as_ptr(), b"nan NAN")?;
    sprintf(0, &mut buf, 100, b"%F\0", &Va::new().f64(1.23));
    c.str(buf.as_ptr(), b"1.230000")?;
    sprintf(0, &mut buf, 100, b"%.30f\0", &Va::new().f64(1.0 / 3.0));
    c.str(buf.as_ptr(), b"0.333333333333333314829616256247")?;
    sprintf(0, &mut buf, 100, b"%.0f %.0f\0", &Va::new().f64(0.5).f64(2.5));
    c.str(buf.as_ptr(), b"1 3")?;
    // _CRT_INTERNAL_PRINTF_STANDARD_ROUNDING (32): exact ties to even.
    let ties = Va::new().f64(0.5).f64(1.5).f64(2.5).f64(0.125);
    sprintf(STANDARD_ROUNDING, &mut buf, 100, b"%.0f %.0f %.0f %.2f\0", &ties);
    c.str(buf.as_ptr(), b"0 2 2 0.12")?;
    sprintf(0, &mut buf, 100, b"%zx %d\0", &Va::new().size(usize::MAX).int(1));
    #[cfg(target_arch = "x86_64")]
    c.str(buf.as_ptr(), b"ffffffffffffffff 1")?;
    #[cfg(target_arch = "x86")]
    c.str(buf.as_ptr(), b"ffffffff 1")?;
    sprintf(0, &mut buf, 100, b"%jd %d\0", &Va::new().i64(i64::MIN).int(1));
    c.str(buf.as_ptr(), b"-9223372036854775808 1")?;
    sprintf(0, &mut buf, 100, b"%I64u %lld\0", &Va::new().i64(-1).i64(-2));
    c.str(buf.as_ptr(), b"18446744073709551615 -2")?;
    let abcd = b"abcd\0";
    let wide_abcd = w::<5>("ABCD");
    sprintf(
        0,
        &mut buf,
        100,
        b"%s %Ts %ls\0",
        &Va::new().ptr(abcd.as_ptr()).ptr(abcd.as_ptr()).ptr(wide_abcd.as_ptr()),
    );
    c.str(buf.as_ptr(), b"abcd abcd ABCD")?;
    sprintf(0, &mut buf, 100, b"%0*2d|% *2d\0", &Va::new().int(1).int(3).int(0).int(7));
    c.str(buf.as_ptr(), b"03| 7")?;

    // Wide: %s is narrow in the standard mode, wide with LEGACY_WIDE.
    let mut wbuf = [0u16; 20];
    let fmt = w::<8>("%s %ls");
    let args = Va::new().ptr(abcd.as_ptr()).ptr(wide_abcd.as_ptr());
    // SAFETY: valid buffers and formats.
    let n =
        unsafe { __stdio_common_vswprintf(0, wbuf.as_mut_ptr(), 20, fmt.as_ptr(), 0, args.list()) };
    c.eq(n.into(), 9)?;
    c.wstr(wbuf.as_ptr(), b"abcd ABCD")?;
    let fmt = w::<8>("%hs %s");
    // SAFETY: as above.
    unsafe {
        __stdio_common_vswprintf(LEGACY_WIDE, wbuf.as_mut_ptr(), 20, fmt.as_ptr(), 0, args.list())
    };
    c.wstr(wbuf.as_ptr(), b"abcd ABCD")?;
    let fmt = w::<12>("muchlonger");
    // SAFETY: as above.
    let n =
        unsafe { __stdio_common_vswprintf(0, wbuf.as_mut_ptr(), 8, fmt.as_ptr(), 0, none.list()) };
    c.eq(n.into(), -2)?;
    c.wstr(wbuf.as_ptr(), b"muchlon")?;
    let fmt = w::<8>("%d%lc%%");
    // SAFETY: as above.
    let n = unsafe {
        __stdio_common_vswprintf(
            0,
            wbuf.as_mut_ptr(),
            20,
            fmt.as_ptr(),
            0,
            Va::new().int(-7).int(0x263a).list(),
        )
    };
    c.eq(n.into(), 4)?;
    c.ok(
        wbuf[..5] == [u16::from(b'-'), u16::from(b'7'), 0x263a, u16::from(b'%'), 0],
        u64::from(wbuf[2]),
    )?;

    // The secure ones: _TRUNCATE returns -1 and keeps what fits.
    let fmt = b"AB%uC\0";
    let args = Va::new().int(123);
    // SAFETY: valid buffers and formats.
    unsafe {
        let p = buf.as_mut_ptr();
        c.eq(
            __stdio_common_vsnprintf_s(0, p, 14, TRUNCATE, fmt.as_ptr(), 0, args.list()).into(),
            6,
        )?;
        c.str(p, b"AB123C")?;
        c.eq(
            __stdio_common_vsnprintf_s(0, p, 7, TRUNCATE, fmt.as_ptr(), 0, args.list()).into(),
            6,
        )?;
        c.eq(
            __stdio_common_vsnprintf_s(0, p, 6, TRUNCATE, fmt.as_ptr(), 0, args.list()).into(),
            -1,
        )?;
        c.str(p, b"AB123")?;
        c.eq(
            __stdio_common_vsnprintf_s(0, p, 1, TRUNCATE, fmt.as_ptr(), 0, args.list()).into(),
            -1,
        )?;
        c.str(p, b"")?;
        c.eq(__stdio_common_vsprintf_s(0, p, 14, fmt.as_ptr(), 0, args.list()).into(), 6)?;
        c.str(p, b"AB123C")?;
        let wfmt = w::<6>("AB%uC");
        let q = wbuf.as_mut_ptr();
        c.eq(
            __stdio_common_vsnwprintf_s(0, q, 14, TRUNCATE, wfmt.as_ptr(), 0, args.list()).into(),
            6,
        )?;
        c.wstr(q, b"AB123C")?;
        c.eq(
            __stdio_common_vsnwprintf_s(0, q, 6, TRUNCATE, wfmt.as_ptr(), 0, args.list()).into(),
            -1,
        )?;
        c.wstr(q, b"AB123")?;
        c.eq(__stdio_common_vswprintf_s(0, q, 14, wfmt.as_ptr(), 0, args.list()).into(), 6)?;
        c.wstr(q, b"AB123C")?;
    }
    Ok(())
}

// ---- scanf ------------------------------------------------------------------------

fn scanf(c: &mut Checks) -> Result<(), u32> {
    let (mut i, mut d, mut s) = (0i32, 0f64, [0u8; 8]);
    let args = Va::new().out(&mut i).ptr(s.as_mut_ptr()).out(&mut d);
    // SAFETY: valid buffers, formats and pointers.
    unsafe {
        let n = __stdio_common_vsscanf(
            0,
            b"12 abc 3.5\0".as_ptr(),
            usize::MAX,
            b"%d %s %lf\0".as_ptr(),
            0,
            args.list(),
        );
        c.eq(n.into(), 3)?;
        c.eq(i.into(), 12)?;
        c.str(s.as_ptr(), b"abc")?;
        c.ok(d == 3.5, d.to_bits())?;
        // The count limits the input.
        let args = Va::new().out(&mut i);
        let n =
            __stdio_common_vsscanf(0, b"123456\0".as_ptr(), 3, b"%d\0".as_ptr(), 0, args.list());
        c.eq(n.into(), 1)?;
        c.eq(i.into(), 123)?;
        // The secure form: a size after the string's pointer.
        let args = Va::new().ptr(s.as_mut_ptr()).int(8).out(&mut i);
        let n = __stdio_common_vsscanf(
            1,
            b"xyz 7\0".as_ptr(),
            usize::MAX,
            b"%s %d\0".as_ptr(),
            0,
            args.list(),
        );
        c.eq(n.into(), 2)?;
        c.str(s.as_ptr(), b"xyz")?;
        c.eq(i.into(), 7)?;
        // Wide: %ls writes UTF-16.
        let mut ws = [0u16; 8];
        let input = w::<8>("42 hi");
        let fmt = w::<8>("%d %ls");
        let args = Va::new().out(&mut i).ptr(ws.as_mut_ptr());
        let n =
            __stdio_common_vswscanf(0, input.as_ptr(), usize::MAX, fmt.as_ptr(), 0, args.list());
        c.eq(n.into(), 2)?;
        c.eq(i.into(), 42)?;
        c.wstr(ws.as_ptr(), b"hi")?;
    }
    Ok(())
}

// ---- Streams -------------------------------------------------------------------

fn streams(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout; the file is ours.
    unsafe {
        c.eq(_fileno(__acrt_iob_func(0)).into(), 0)?;
        c.eq(_fileno(__acrt_iob_func(1)).into(), 1)?;
        c.eq(_fileno(__acrt_iob_func(2)).into(), 2)?;

        let mut path = [0u16; 300];
        let n = GetTempPathW(260, path.as_mut_ptr()) as usize;
        c.ok(n > 0 && n < 250, n as u64)?;
        for (i, ch) in "gala_ucrt_test.txt".bytes().enumerate() {
            path[n + i] = u16::from(ch);
        }
        DeleteFileW(path.as_ptr());
        let mut f = null_mut();
        c.eq(_wfopen_s(&mut f, path.as_ptr(), w::<4>("w+b").as_ptr()).into(), 0)?;
        c.ok(!f.is_null(), 0)?;
        c.ok(_fileno(f) > 2, _fileno(f) as u64)?;
        _lock_file(f);
        c.ok(fputs(b"hello\nworld\n\0".as_ptr(), f) >= 0, 0)?;
        _unlock_file(f);
        c.eq(fflush(f).into(), 0)?;
        c.eq(_fseeki64(f, 0, 0).into(), 0)?;
        let mut line = [0u8; 32];
        c.ok(fgets(line.as_mut_ptr(), 32, f) == line.as_mut_ptr(), 0)?;
        c.str(line.as_ptr(), b"hello\n")?;
        let mut pos = 0i64;
        c.eq(fgetpos(f, &mut pos).into(), 0)?;
        c.eq(pos, 6)?;
        c.eq(fgetc(f).into(), i64::from(b'w'))?;
        c.eq(ungetc(i32::from(b'W'), f).into(), i64::from(b'W'))?;
        c.eq(fgetc(f).into(), i64::from(b'W'))?;
        c.eq(fsetpos(f, &pos).into(), 0)?;
        c.eq(fgetc(f).into(), i64::from(b'w'))?;
        let (mut base, mut ptr, mut count) = (null_mut(), null_mut(), null_mut());
        c.eq(_get_stream_buffer_pointers(f, &mut base, &mut ptr, &mut count).into(), 0)?;
        c.ok(!base.is_null() && !ptr.is_null() && !count.is_null(), 0)?;
        c.eq(fclose(f).into(), 0)?;

        let f = _wfsopen(path.as_ptr(), w::<4>("rb").as_ptr(), 0x40);
        c.ok(!f.is_null(), 0)?;
        c.eq(setvbuf(f, null_mut(), 4, 0).into(), 0)?;
        let mut data = [0u8; 8];
        c.eq(fread(data.as_mut_ptr(), 1, 5, f) as i64, 5)?;
        c.ok(&data[..5] == b"hello", 0)?;
        c.eq(fclose(f).into(), 0)?;
        c.ok(DeleteFileW(path.as_ptr()) != 0, 0)?;
        let mut f = null_mut();
        c.eq(_wfopen_s(&mut f, path.as_ptr(), w::<4>("rb").as_ptr()).into(), ENOENT.into())?;
        c.ok(f.is_null(), 0)?;
    }
    Ok(())
}

// ---- Strings ---------------------------------------------------------------------

fn strings(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout.
    unsafe {
        let mut buf = [0u8; 16];
        let p = buf.as_mut_ptr();
        c.eq(strcpy_s(p, 8, b"abc\0".as_ptr()).into(), 0)?;
        c.str(p, b"abc")?;
        c.eq(strncpy_s(p, 8, b"abcdef\0".as_ptr(), 3).into(), 0)?;
        c.str(p, b"abc")?;
        c.eq(strncpy_s(p, 4, b"abcdef\0".as_ptr(), TRUNCATE).into(), STRUNCATE.into())?;
        c.str(p, b"abc")?;
        let mut wbuf = [0u16; 16];
        let q = wbuf.as_mut_ptr();
        c.eq(wcscpy_s(q, 8, w::<4>("ab").as_ptr()).into(), 0)?;
        c.eq(wcsncat_s(q, 8, w::<8>("cdef").as_ptr(), 2).into(), 0)?;
        c.wstr(q, b"abcd")?;
        c.eq(wcsncpy_s(q, 8, w::<8>("xyz").as_ptr(), 2).into(), 0)?;
        c.wstr(q, b"xy")?;
        let src = w::<4>("123");
        c.eq(wmemcpy_s(q, 4, src.as_ptr(), 3).into(), 0)?;
        c.eq(wmemcpy_s(q, 2, src.as_ptr(), 3).into(), ERANGE.into())?;
        let mut text = w::<8>("a,b,,c");
        let mut ctx = null_mut();
        let sep = w::<2>(",");
        c.wstr(wcstok_s(text.as_mut_ptr(), sep.as_ptr(), &mut ctx), b"a")?;
        c.wstr(wcstok_s(null_mut(), sep.as_ptr(), &mut ctx), b"b")?;
        c.wstr(wcstok_s(null_mut(), sep.as_ptr(), &mut ctx), b"c")?;
        c.ok(wcstok_s(null_mut(), sep.as_ptr(), &mut ctx).is_null(), 0)?;
        c.eq(strnlen(b"abc\0".as_ptr(), 2) as i64, 2)?;
        c.eq(strnlen(b"abc\0".as_ptr(), 9) as i64, 3)?;
        c.eq(wcsnlen(w::<4>("abc").as_ptr(), 9) as i64, 3)?;
        c.eq(__strncnt(b"abc\0".as_ptr(), 5) as i64, 3)?;
        c.eq(__strncnt(b"abc\0".as_ptr(), 2) as i64, 2)?;

        c.eq(_itow_s(-12, q, 8, 10).into(), 0)?;
        c.wstr(q, b"-12")?;
        c.eq(_ultow_s(8, q, 8, 2).into(), 0)?;
        c.wstr(q, b"1000")?;
        c.eq(_i64tow_s(-1, q, 8, 10).into(), 0)?;
        c.wstr(q, b"-1")?;
        c.eq(_ui64tow_s(255, q, 8, 16).into(), 0)?;
        c.wstr(q, b"ff")?;
        c.eq(_ui64tow_s(255, q, 2, 10).into(), ERANGE.into())?;

        let mut end = null();
        c.eq(strtoll(b"-9223372036854775808x\0".as_ptr(), &mut end, 10), i64::MIN)?;
        c.eq((*end).into(), i64::from(b'x'))?;
        c.eq(strtoull(b"0xffffffffffffffff\0".as_ptr(), null_mut(), 16) as i64, -1)?;
        *_errno() = 0;
        c.eq(strtol(b"99999999999\0".as_ptr(), null_mut(), 10).into(), i32::MAX.into())?;
        c.eq((*_errno()).into(), ERANGE.into())?;
        let mut wend = null();
        let num = w::<8>("-123x");
        c.eq(_wcstoi64(num.as_ptr(), &mut wend, 10), -123)?;
        c.eq((*wend).into(), i64::from(b'x'))?;
        c.eq(_wcstoui64(w::<4>("ff").as_ptr(), null_mut(), 16) as i64, 255)?;
        c.eq(wcstoull(w::<8>("12345").as_ptr(), null_mut(), 0) as i64, 12345)?;
        let f = strtof(b"0.5\0".as_ptr(), null_mut());
        c.ok(f == 0.5, u64::from(f.to_bits()))?;
        let d = wcstod(w::<8>("2.5e2").as_ptr(), null_mut());
        c.ok(d == 250.0, d.to_bits())?;
        let d = _wtof(w::<8>(" -1.25").as_ptr());
        c.ok(d == -1.25, d.to_bits())?;
        c.eq(_wtoi(w::<8>("  77z").as_ptr()).into(), 77)?;
        c.eq(_wtoi64(w::<16>("-5000000000").as_ptr()), -5_000_000_000)?;
        c.eq(atol(b"-42\0".as_ptr()).into(), -42)?;

        let mut state = MbState::default();
        let mut wc = 0u16;
        c.eq(mbrtowc(&mut wc, b"A\0".as_ptr(), 1, &mut state) as i64, 1)?;
        c.eq(wc.into(), i64::from(b'A'))?;
        c.eq(mbrlen(b"B\0".as_ptr(), 1, &mut state) as i64, 1)?;
        let mut out = [0u8; 8];
        c.eq(wcrtomb(out.as_mut_ptr(), u16::from(b'C'), &mut state) as i64, 1)?;
        c.eq(out[0].into(), i64::from(b'C'))?;
        let mut converted = 0usize;
        c.eq(mbstowcs_s(&mut converted, q, 8, b"abc\0".as_ptr(), TRUNCATE).into(), 0)?;
        c.eq(converted as i64, 4)?;
        c.wstr(q, b"abc")?;
        c.eq(wcstombs_s(&mut converted, p, 8, w::<4>("xyz").as_ptr(), TRUNCATE).into(), 0)?;
        c.eq(converted as i64, 4)?;
        c.str(p, b"xyz")?;
        let mut msg = [0u8; 64];
        c.eq(strerror_s(msg.as_mut_ptr(), 64, ENOENT).into(), 0)?;
        c.str(msg.as_ptr(), b"No such file or directory")?;
    }
    case_and_paths(c)
}

/// `_wcsupr_s`/`_wcslwr_s` (Wine's msvcrt string.c test__wcsupr_s and
/// test__wcslwr_s), `_wmakepath_s` (dir.c test_makepath_s: the drive's
/// letter and a colon, a backslash after the directory, a dot before the
/// extension; ERANGE writes what fits before the terminator's place,
/// then empties the buffer), `_wsplitpath_s` (documented: the drive with
/// its colon, the directory with its last separator, the extension from
/// the last dot; a buffer too small empties every one, ERANGE),
/// `_resetstkoflw` (nonzero) and `_get_heap_handle` (a heap handle).
fn case_and_paths(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout (the invalid ones are what the
    // functions check, with our handler installed).
    unsafe {
        let mixed = w::<16>("MiXeDlowerUPPER");
        let mut buf = [0u16; 32];
        let b = buf.as_mut_ptr();
        *_errno() = 0;
        c.eq(_wcsupr_s(null_mut(), 0).into(), EINVAL.into())?;
        c.eq((*_errno()).into(), EINVAL.into())?;
        buf[0] = u16::from(b'x');
        c.eq(_wcsupr_s(b, 0).into(), EINVAL.into())?;
        c.eq(buf[0].into(), 0)?;
        c.eq(_wcsupr_s(b, 1).into(), 0)?;
        buf[0] = u16::from(b'x');
        c.eq(_wcsupr_s(b, 1).into(), EINVAL.into())?;
        c.eq(buf[0].into(), 0)?;
        buf[..16].copy_from_slice(&mixed);
        c.eq(_wcsupr_s(b, 16).into(), 0)?;
        c.wstr(b, b"MIXEDLOWERUPPER")?;
        buf[..16].copy_from_slice(&mixed);
        c.eq(_wcsupr_s(b, 15).into(), EINVAL.into())?;
        c.eq(buf[0].into(), 0)?;
        buf[..16].copy_from_slice(&mixed);
        c.eq(_wcslwr_s(b, 32).into(), 0)?;
        c.wstr(b, b"mixedlowerupper")?;
        c.eq(_wcslwr_s(null_mut(), 32).into(), EINVAL.into())?;

        // _wmakepath_s.
        let (drive, dir, file, ext) =
            (w::<4>("c:\\"), w::<4>("dir"), w::<8>("file"), w::<4>("ext"));
        c.eq(
            _wmakepath_s(b, 32, drive.as_ptr(), dir.as_ptr(), file.as_ptr(), ext.as_ptr()).into(),
            0,
        )?;
        c.wstr(b, b"c:dir\\file.ext")?;
        let (dir2, ext2) = (w::<8>("\\dir\\"), w::<8>(".ext"));
        c.eq(_wmakepath_s(b, 32, null(), dir2.as_ptr(), null(), ext2.as_ptr()).into(), 0)?;
        c.wstr(b, b"\\dir\\.ext")?;
        c.eq(_wmakepath_s(b, 32, null(), null(), null(), null()).into(), 0)?;
        c.wstr(b, b"")?;
        c.eq(_wmakepath_s(null_mut(), 0, null(), null(), null(), null()).into(), EINVAL.into())?;
        c.eq(_wmakepath_s(b, 0, null(), null(), null(), null()).into(), EINVAL.into())?;
        // "c:", "d\\", "file", "ext" into 5 characters: "\0:d\\" then the
        // untouched rest.
        buf = [u16::from(b'X'); 32];
        let d = w::<4>("d\\");
        c.eq(
            _wmakepath_s(b, 5, drive.as_ptr(), d.as_ptr(), file.as_ptr(), ext.as_ptr()).into(),
            ERANGE.into(),
        )?;
        c.eq((*_errno()).into(), ERANGE.into())?;
        c.ok(
            buf[..6]
                == [
                    0,
                    u16::from(b':'),
                    u16::from(b'd'),
                    u16::from(b'\\'),
                    u16::from(b'X'),
                    u16::from(b'X'),
                ],
            u64::from(buf[1]),
        )?;

        // _wsplitpath_s.
        let path = w::<32>("c:\\dir\\sub/file.name.ext");
        let (mut dr, mut di, mut fname, mut ex) = ([7u16; 4], [7u16; 32], [7u16; 32], [7u16; 8]);
        c.eq(
            _wsplitpath_s(
                path.as_ptr(),
                dr.as_mut_ptr(),
                4,
                di.as_mut_ptr(),
                32,
                fname.as_mut_ptr(),
                32,
                ex.as_mut_ptr(),
                8,
            )
            .into(),
            0,
        )?;
        c.wstr(dr.as_ptr(), b"c:")?;
        c.wstr(di.as_ptr(), b"\\dir\\sub/")?;
        c.wstr(fname.as_ptr(), b"file.name")?;
        c.wstr(ex.as_ptr(), b".ext")?;
        let plain = w::<8>("file");
        c.eq(
            _wsplitpath_s(
                plain.as_ptr(),
                dr.as_mut_ptr(),
                4,
                null_mut(),
                0,
                fname.as_mut_ptr(),
                32,
                ex.as_mut_ptr(),
                8,
            )
            .into(),
            0,
        )?;
        c.wstr(dr.as_ptr(), b"")?;
        c.wstr(fname.as_ptr(), b"file")?;
        c.wstr(ex.as_ptr(), b"")?;
        // An extension too long for its buffer: every buffer emptied.
        c.eq(
            _wsplitpath_s(
                path.as_ptr(),
                dr.as_mut_ptr(),
                4,
                di.as_mut_ptr(),
                32,
                fname.as_mut_ptr(),
                32,
                ex.as_mut_ptr(),
                4,
            )
            .into(),
            ERANGE.into(),
        )?;
        c.ok(dr[0] == 0 && di[0] == 0 && fname[0] == 0 && ex[0] == 0, u64::from(di[0]))?;
        c.eq(
            _wsplitpath_s(null(), dr.as_mut_ptr(), 4, null_mut(), 0, null_mut(), 0, null_mut(), 0)
                .into(),
            EINVAL.into(),
        )?;
        c.eq(
            _wsplitpath_s(
                path.as_ptr(),
                dr.as_mut_ptr(),
                0,
                null_mut(),
                0,
                null_mut(),
                0,
                null_mut(),
                0,
            )
            .into(),
            EINVAL.into(),
        )?;

        c.ok(_resetstkoflw() != 0, 0)?;
        c.ok(_get_heap_handle() != 0, 0)?;
    }
    Ok(())
}

// ---- Locale and math --------------------------------------------------------------

fn locale_math(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout.
    unsafe {
        c.str(setlocale(0, null()), b"C")?;
        c.eq(___mb_cur_max_func().into(), 1)?;
        let ctype = __pctype_func();
        c.ok(*ctype.add(usize::from(b'A')) & 0x1 != 0, 0)?;
        c.ok(*ctype.add(usize::from(b'a')) & 0x2 != 0, 0)?;
        c.ok(*ctype.add(usize::from(b'5')) & 0x4 != 0, 0)?;
        c.ok(*ctype.add(usize::from(b' ')) & 0x8 != 0, 0)?;
        c.str((*localeconv()).decimal_point, b".")?;
        c.eq(_configthreadlocale(0).into(), 2)?;

        c.eq(_dclass(1.0).into(), -1)?;
        c.eq(_dclass(0.0).into(), 0)?;
        c.eq(_dclass(f64::INFINITY).into(), 1)?;
        c.eq(_dclass(f64::NAN).into(), 2)?;
        c.eq(_dclass(5e-324).into(), -2)?;
        c.eq(_fdclass(1.0).into(), -1)?;
        c.eq(_fdclass(f32::INFINITY).into(), 1)?;
        c.ok(_dsign(-1.0) != 0, 0)?;
        c.eq(_dsign(1.0).into(), 0)?;
        c.ok(round(2.5) == 3.0 && round(-2.5) == -3.0 && round(0.4) == 0.0, 0)?;
        let mut cw = 0;
        c.eq(_controlfp_s(&mut cw, 0, 0).into(), 0)?;
        // Exceptions masked: _MCW_EM's bits all set.
        c.eq((cw & 0x1f).into(), 0x1f)?;
        // C99: complex numbers, comparison, the exception flags.
        c.ok(cabs(Dcomplex { re: 3.0, im: 4.0 }) == 5.0, 0)?;
        c.ok(cimag(Dcomplex { re: 3.0, im: 4.0 }) == 4.0, 0)?;
        let e = cexp(Dcomplex { re: 0.0, im: core::f64::consts::PI });
        c.ok((e.re + 1.0).abs() < 1e-15 && e.im.abs() < 1e-15, e.re.to_bits())?;
        c.eq(_dpcomp(1.0, 2.0).into(), 1)?;
        c.eq(_dpcomp(2.0, 2.0).into(), 2)?;
        c.eq(_dpcomp(f64::NAN, 2.0).into(), 0)?;
        c.eq(feclearexcept(0x1f).into(), 0)?;
        c.eq(fetestexcept(0x1f).into(), 0)?;
        let zero = core::hint::black_box(0.0f64);
        c.ok(core::hint::black_box(1.0 / zero).is_infinite(), 0)?;
        c.eq(fetestexcept(0x1f).into(), 8)?; // FE_DIVBYZERO
        c.eq(feclearexcept(8).into(), 0)?;
        c.eq(fetestexcept(0x1f).into(), 0)?;
        c.eq(btowc(i32::from(b'A')).into(), 65)?;

        #[cfg(target_arch = "x86")]
        {
            let e = sse2(_libm_sse2_exp_precise as *const () as usize, 1.0, 0.0);
            c.ok((e - core::f64::consts::E).abs() < 1e-15, e.to_bits())?;
            let l = sse2(_libm_sse2_log_precise as *const () as usize, core::f64::consts::E, 0.0);
            c.ok((l - 1.0).abs() < 1e-15, l.to_bits())?;
            let l = sse2(_libm_sse2_log10_precise as *const () as usize, 1000.0, 0.0);
            c.ok((l - 3.0).abs() < 1e-15, l.to_bits())?;
            let p = sse2(_libm_sse2_pow_precise as *const () as usize, 2.0, 10.0);
            c.ok(p == 1024.0, p.to_bits())?;
        }

        let mut tm = Tm::default();
        let t = 365 * 86400i64;
        c.eq(_gmtime64_s(&mut tm, &t).into(), 0)?;
        c.ok(
            tm.year == 71 && tm.mon == 0 && tm.mday == 1 && tm.yday == 0 && tm.hour == 0,
            tm.year as u64,
        )?;
        c.eq(tm.wday.into(), 5)?;
        c.eq(_localtime64_s(&mut tm, &t).into(), 0)?;
        c.ok(tm.year == 70 || tm.year == 71, tm.year as u64)?;
        c.ok(_difftime64(10, 4) == 6.0, 0)?;
    }
    Ok(())
}

/// Calls a `_libm_sse2_*` function: the arguments in xmm0 and xmm1, the
/// result in xmm0.
#[cfg(target_arch = "x86")]
fn sse2(f: usize, x: f64, y: f64) -> f64 {
    // x, y in; the result over x.
    let mut v = [x, y];
    // SAFETY: `f` takes its arguments in xmm0/xmm1 and returns in xmm0,
    // clobbering what a cdecl function may (eax, ecx, edx, xmm0-7, st);
    // edi is preserved across the call.
    unsafe {
        core::arch::asm!(
            "movsd xmm0, qword ptr [edi]",
            "movsd xmm1, qword ptr [edi + 8]",
            "call eax",
            "movsd qword ptr [edi], xmm0",
            in("edi") v.as_mut_ptr(),
            inout("eax") f => _,
            out("ecx") _, out("edx") _,
            out("xmm0") _, out("xmm1") _, out("xmm2") _, out("xmm3") _,
            out("xmm4") _, out("xmm5") _, out("xmm6") _, out("xmm7") _,
        );
    }
    v[0]
}

// ---- Heap, threads, utility ----------------------------------------------------------

extern "system" fn thread_main(arg: *mut c_void) -> u32 {
    // SAFETY: ends this thread with the code we were given.
    unsafe { _endthreadex(arg as u32) }
}

extern "C" fn compare_u32(a: *const c_void, b: *const c_void) -> i32 {
    // SAFETY: qsort passes pointers to the array's u32s.
    let (a, b) = unsafe { (*a.cast::<u32>(), *b.cast::<u32>()) };
    a.cmp(&b) as i32
}

fn heap_misc(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout.
    unsafe {
        let p = malloc(10);
        c.ok(!p.is_null(), 0)?;
        c.eq(_msize(p) as i64, 10)?;
        free(p);

        let mut id = 0;
        let h = _beginthreadex(0, 0, thread_main, 7 as *mut c_void, 0, &mut id);
        c.ok(h != 0 && id != 0, 0)?;
        c.eq(WaitForSingleObject(h, 10_000).into(), 0)?;
        let mut code = 0;
        GetExitCodeThread(h, &mut code);
        c.eq(code.into(), 7)?;
        CloseHandle(h);

        srand(1);
        c.eq(rand().into(), 41)?;
        c.eq(rand().into(), 18467)?;
        let mut v = 0;
        c.eq(rand_s(&mut v).into(), 0)?;
        let mut a = [5u32, 1, 4, 2, 3];
        qsort(a.as_mut_ptr().cast(), 5, 4, compare_u32);
        c.ok(a == [1, 2, 3, 4, 5], u64::from(a[0]))?;
        c.eq(_byteswap_ulong(0x1234_5678).into(), 0x7856_3412)?;
        c.eq(_byteswap_ushort(0x1234).into(), 0x3412)?;
    }
    Ok(())
}

// ---- Exit ---------------------------------------------------------------------------

/// What ran at exit, in order: 1 the TLS callback, 2 and 3 the atexit
/// functions (registered 3 first, then 2).
static EXIT_ORDER: AtomicU32 = AtomicU32::new(0);

extern "system" fn tls_exit(_module: usize, reason: u32, _reserved: usize) {
    // DLL_PROCESS_DETACH.
    if reason == 0 {
        EXIT_ORDER.store(EXIT_ORDER.load(Ordering::Relaxed) * 10 + 1, Ordering::Relaxed);
    }
}

extern "C" fn at_exit_2() {
    EXIT_ORDER.store(EXIT_ORDER.load(Ordering::Relaxed) * 10 + 2, Ordering::Relaxed);
}

extern "C" fn at_exit_3() {
    let order = EXIT_ORDER.load(Ordering::Relaxed) * 10 + 3;
    if order == 123 {
        Line::new().s(NAME.as_bytes()).s(b": atexit ran in order\r\n").print();
    } else {
        Line::new().s(NAME.as_bytes()).s(b": atexit order ").dec(order.into()).s(b"\r\n").print();
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    startup(&mut c)?;
    invalid_parameter(&mut c)?;
    printf(&mut c)?;
    scanf(&mut c)?;
    streams(&mut c)?;
    strings(&mut c)?;
    locale_math(&mut c)?;
    heap_misc(&mut c)?;
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    match run() {
        Ok(()) => {
            Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
            // SAFETY: registered once; plain functions.
            unsafe {
                _register_thread_local_exe_atexit_callback(tls_exit);
                _crt_atexit(at_exit_3);
                _crt_atexit(at_exit_2);
                exit(0)
            }
        }
        // SAFETY: ends the process.
        Err(n) => unsafe { ExitProcess(n) },
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
