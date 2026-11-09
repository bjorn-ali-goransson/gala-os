//! The Universal C Runtime's own functions (gala's ucrtbase.dll,
//! ARCHITECTURE "The UCRT"): built only for the `ucrtbase` package
//! (`cfg(ucrt)`), on top of msvcrt's implementation, which the same
//! build carries (so ucrtbase.dll exports both and has its own state).
//!
//! Written from Microsoft's documentation (learn.microsoft.com, "C runtime
//! library reference" and "CRT initialization") with Wine's ucrtbase
//! (dlls/msvcrt, dlls/ucrtbase and their tests) as documentation of the
//! details the pages leave open (THIRD_PARTY.md).
//!
//! This module: the start-up interface that Visual C++'s static start-up
//! code (in every program, vcstartup) calls: arguments and environment,
//! `_initterm_e`, onexit tables, the exit sequence, the app type and file
//! mode, the new-handler mode, the invalid-parameter handler and
//! `terminate`. The rest: `stdio.rs` (the `__stdio_common_*` functions
//! and streams), `string.rs`, `locale.rs`, `math.rs`, `time.rs`.

mod compat;
mod locale;
mod math;
mod stdio;
mod string;
mod time;

use crate::heap::{free, malloc, realloc};
use crate::{Lock, stdio::write_stderr};
use core::ffi::c_void;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetCommandLineW() -> *const u16;
    fn GetCurrentProcess() -> *mut c_void;
    fn TerminateProcess(process: *mut c_void, code: u32) -> i32;
}

pub(crate) const EINVAL: i32 = 22;
pub(crate) const ERANGE: i32 = 34;
pub(crate) const STRUNCATE: i32 = 80;
/// `_TRUNCATE` ((size_t)-1): copy what fits.
pub(crate) const TRUNCATE: usize = usize::MAX;

// ---- Arguments and environment ---------------------------------------------

/// `_configure_narrow_argv(mode)`: builds `__argc`/`__argv` (mode 1
/// `_crt_argv_unexpanded_arguments`; 2, wildcard expansion, is not done:
/// gala's msvcrt does not expand either); 0.
#[unsafe(no_mangle)]
pub extern "C" fn _configure_narrow_argv(mode: i32) -> i32 {
    if mode == 2 {
        crate::missing(b"ucrtbase.dll!_configure_narrow_argv(expanded)", 2);
    }
    let (mut argc, mut argv, mut env) = (0, null_mut(), null_mut());
    crate::__getmainargs(&mut argc, &mut argv, &mut env, 0, null_mut())
}

/// `_configure_wide_argv(mode)`: builds `__argc`/`__wargv`; 0.
#[unsafe(no_mangle)]
pub extern "C" fn _configure_wide_argv(mode: i32) -> i32 {
    if mode == 2 {
        crate::missing(b"ucrtbase.dll!_configure_wide_argv(expanded)", 2);
    }
    let (mut argc, mut argv, mut env) = (0, null_mut(), null_mut());
    let r = crate::wide::__wgetmainargs(&mut argc, &mut argv, &mut env, 0, null_mut());
    // SAFETY: the runtime's own static, set once the arguments exist.
    unsafe {
        if _wenviron.is_null() {
            _wenviron = env;
        }
    }
    r
}

/// `_initialize_narrow_environment()`: builds `_environ`; 0.
#[unsafe(no_mangle)]
pub extern "C" fn _initialize_narrow_environment() -> i32 {
    _configure_narrow_argv(1)
}

/// `_initialize_wide_environment()`: builds `_wenviron`; 0.
#[unsafe(no_mangle)]
pub extern "C" fn _initialize_wide_environment() -> i32 {
    _configure_wide_argv(1)
}

/// `_wenviron`: the wide environment (built by `_initialize_wide_environment`).
#[unsafe(no_mangle)]
pub static mut _wenviron: *mut *mut u16 = null_mut();

#[unsafe(no_mangle)]
pub extern "C" fn __p___argc() -> *mut i32 {
    &raw mut crate::__argc
}

#[unsafe(no_mangle)]
pub extern "C" fn __p___argv() -> *mut *mut *mut u8 {
    &raw mut crate::__argv
}

#[unsafe(no_mangle)]
pub extern "C" fn __p___wargv() -> *mut *mut *mut u16 {
    &raw mut crate::wide::__wargv
}

#[unsafe(no_mangle)]
pub extern "C" fn __p__environ() -> *mut *mut *mut u8 {
    &raw mut crate::_environ
}

#[unsafe(no_mangle)]
pub extern "C" fn __p__wenviron() -> *mut *mut *mut u16 {
    &raw mut _wenviron
}

#[unsafe(no_mangle)]
pub extern "C" fn __p__acmdln() -> *mut *mut u8 {
    &raw mut crate::_acmdln
}

/// `_get_initial_narrow_environment()`: the environment main gets.
#[unsafe(no_mangle)]
pub extern "C" fn _get_initial_narrow_environment() -> *mut *mut u8 {
    _initialize_narrow_environment();
    // SAFETY: set by the call above.
    unsafe { crate::__initenv }
}

/// `_get_initial_wide_environment()`: the environment wmain gets.
#[unsafe(no_mangle)]
pub extern "C" fn _get_initial_wide_environment() -> *mut *mut u16 {
    _initialize_wide_environment();
    // SAFETY: set by the call above.
    unsafe { crate::wide::__winitenv }
}

/// Where WinMain's command line starts in `line`: after the program name
/// (a double quote toggles quoting; outside quotes a blank or control
/// character ends it), then after the blanks that follow.
fn winmain_start(line: &[u16]) -> usize {
    let (mut i, mut quoted) = (0, false);
    while i < line.len() && (line[i] > 0x20 || (quoted && line[i] != 0)) {
        if line[i] == u16::from(b'"') {
            quoted = !quoted;
        }
        i += 1;
    }
    while i < line.len() && line[i] != 0 && line[i] <= 0x20 {
        i += 1;
    }
    i
}

/// `_get_wide_winmain_command_line()`: the command line after the
/// program name (in GetCommandLineW's buffer).
#[unsafe(no_mangle)]
pub extern "C" fn _get_wide_winmain_command_line() -> *const u16 {
    // SAFETY: the PEB's NUL-terminated command line.
    unsafe {
        let p = GetCommandLineW();
        let line = core::slice::from_raw_parts(p, crate::wide::wlen(p));
        p.add(winmain_start(line))
    }
}

/// `_get_narrow_winmain_command_line()`: the same in the narrow command
/// line (`_acmdln`, made when the DLL loaded).
#[unsafe(no_mangle)]
pub extern "C" fn _get_narrow_winmain_command_line() -> *const u8 {
    // SAFETY: `_acmdln`, NUL-terminated, set at DLL load.
    unsafe {
        let p = crate::_acmdln;
        if p.is_null() {
            return b"\0".as_ptr();
        }
        let n = crate::strlen(p);
        // Bytes as units: the rule only looks at ASCII.
        let mut i = 0;
        let mut quoted = false;
        while i < n && (*p.add(i) > 0x20 || quoted) {
            if *p.add(i) == b'"' {
                quoted = !quoted;
            }
            i += 1;
        }
        while i < n && *p.add(i) <= 0x20 {
            i += 1;
        }
        p.add(i)
    }
}

// ---- Initializers, onexit tables, exit --------------------------------------------

/// `_initterm_e(begin, end)`: calls each non-null `int (*)(void)` in the
/// table; stops at the first nonzero result and returns it (0 if none).
#[unsafe(no_mangle)]
pub extern "C" fn _initterm_e(begin: *const usize, end: *const usize) -> i32 {
    let mut p = begin;
    while p < end {
        // SAFETY: the table holds function pointers or zeros.
        unsafe {
            if *p != 0 {
                let f: extern "C" fn() -> i32 = core::mem::transmute(*p);
                let r = f();
                if r != 0 {
                    return r;
                }
            }
            p = p.add(1);
        }
    }
    0
}

/// `_onexit_table_t` (corecrt_startup.h): an array of function pointers,
/// `first..last` used, `first..end` allocated. gala keeps the pointers
/// plain (Windows encodes them with the process cookie; programs only
/// compare them).
#[repr(C)]
pub struct OnexitTable {
    first: *mut usize,
    last: *mut usize,
    end: *mut usize,
}

static ONEXIT_LOCK: Lock = Lock::new();

/// `_initialize_onexit_table(table)`: an empty table (first == last ==
/// end), unless it already holds an allocation (first != end), which is
/// kept; -1 for NULL.
#[unsafe(no_mangle)]
pub extern "C" fn _initialize_onexit_table(table: *mut OnexitTable) -> i32 {
    if table.is_null() {
        return -1;
    }
    // SAFETY: the caller's table.
    unsafe {
        if (*table).first == (*table).end {
            (*table).first = null_mut();
            (*table).last = null_mut();
            (*table).end = null_mut();
        }
    }
    0
}

/// `_register_onexit_function(table, f)`: appends `f` (NULL too); 0, or
/// -1 for a NULL table or no memory.
#[unsafe(no_mangle)]
pub extern "C" fn _register_onexit_function(table: *mut OnexitTable, f: usize) -> i32 {
    if table.is_null() {
        return -1;
    }
    ONEXIT_LOCK.with(|| {
        // SAFETY: the caller's table, which this runtime manages.
        unsafe {
            let t = &mut *table;
            if t.last == t.end {
                let used = if t.first.is_null() { 0 } else { t.last.offset_from(t.first) as usize };
                let grown = (used * 2).max(32);
                let p = realloc(t.first.cast(), grown * size_of::<usize>()).cast::<usize>();
                if p.is_null() {
                    return -1;
                }
                t.first = p;
                t.last = p.add(used);
                t.end = p.add(grown);
            }
            *t.last = f;
            t.last = t.last.add(1);
        }
        0
    })
}

/// `_execute_onexit_table(table)`: calls the functions last registered
/// first (NULLs skipped) and empties the table; 0, or -1 for NULL.
#[unsafe(no_mangle)]
pub extern "C" fn _execute_onexit_table(table: *mut OnexitTable) -> i32 {
    if table.is_null() {
        return -1;
    }
    // Taken out of the table first: a function may register more.
    let (first, last) = ONEXIT_LOCK.with(|| {
        // SAFETY: the caller's table.
        unsafe {
            let t = &mut *table;
            let taken = (t.first, t.last);
            (t.first, t.last, t.end) = (null_mut(), null_mut(), null_mut());
            taken
        }
    });
    if first.is_null() {
        return 0;
    }
    let mut p = last;
    while p > first {
        // SAFETY: inside the taken array.
        unsafe {
            p = p.sub(1);
            if *p != 0 {
                let f: extern "C" fn() -> i32 = core::mem::transmute(*p);
                f();
            }
        }
    }
    free(first.cast());
    0
}

/// The process's `at_quick_exit` functions.
static mut QUICK_EXIT: OnexitTable =
    OnexitTable { first: null_mut(), last: null_mut(), end: null_mut() };

/// `_crt_atexit(f)`: the program's atexit function (run by `exit`).
#[unsafe(no_mangle)]
pub extern "C" fn _crt_atexit(f: usize) -> i32 {
    crate::atexit(f)
}

/// `_crt_at_quick_exit(f)`: run by `quick_exit`.
#[unsafe(no_mangle)]
pub extern "C" fn _crt_at_quick_exit(f: usize) -> i32 {
    _register_onexit_function(&raw mut QUICK_EXIT, f)
}

/// `quick_exit(code)`: the at_quick_exit functions, then `_exit`.
#[unsafe(no_mangle)]
pub extern "C" fn quick_exit(code: i32) -> ! {
    _execute_onexit_table(&raw mut QUICK_EXIT);
    crate::_exit(code)
}

/// The program's thread_local destructors' callback (a TLS callback).
static TLS_EXIT_CALLBACK: AtomicUsize = AtomicUsize::new(0);

/// `_register_thread_local_exe_atexit_callback(f)`: `exit` calls `f`
/// (with DLL_PROCESS_DETACH) before the atexit functions.
#[unsafe(no_mangle)]
pub extern "C" fn _register_thread_local_exe_atexit_callback(f: usize) {
    TLS_EXIT_CALLBACK.store(f, Ordering::Relaxed);
}

/// What `exit` and `_cexit` do before the atexit functions: the
/// thread_local destructors (once).
pub(crate) fn before_exit_handlers() {
    let f = TLS_EXIT_CALLBACK.swap(0, Ordering::Relaxed);
    if f != 0 {
        // SAFETY: registered as a TLS callback (module, reason, reserved).
        let f: extern "system" fn(usize, u32, usize) = unsafe { core::mem::transmute(f) };
        f(0, 0, 0); // DLL_PROCESS_DETACH
    }
}

/// `_c_exit()`: the end-of-program cleanup without the atexit functions,
/// returning to the caller (nothing to do here).
#[unsafe(no_mangle)]
pub extern "C" fn _c_exit() {}

/// `_seh_filter_exe(code, pointers)`: the exception filter of the
/// program's start-up code: a C signal handler for the exception, if one
/// were installed (signal() installs none here), else the search goes on.
#[unsafe(no_mangle)]
pub extern "C" fn _seh_filter_exe(code: u32, pointers: *mut c_void) -> i32 {
    crate::_XcptFilter(code, pointers)
}

/// `_seh_filter_dll`: as `_seh_filter_exe`, for a DLL's start-up.
#[unsafe(no_mangle)]
pub extern "C" fn _seh_filter_dll(code: u32, pointers: *mut c_void) -> i32 {
    crate::_XcptFilter(code, pointers)
}

// ---- Process settings --------------------------------------------------------------

static APP_TYPE: AtomicI32 = AtomicI32::new(0);

/// `_set_app_type(type)`: console (1) or GUI (2), for error reports.
#[unsafe(no_mangle)]
pub extern "C" fn _set_app_type(kind: i32) {
    APP_TYPE.store(kind, Ordering::Relaxed);
}

/// `_query_app_type()`.
#[unsafe(no_mangle)]
pub extern "C" fn _query_app_type() -> i32 {
    APP_TYPE.load(Ordering::Relaxed)
}

/// `_set_fmode(mode)`: the default file translation mode, _O_TEXT
/// (0x4000) or _O_BINARY (0x8000); EINVAL (and the invalid-parameter
/// handler) for anything else.
#[unsafe(no_mangle)]
pub extern "C" fn _set_fmode(mode: i32) -> i32 {
    if mode != 0x4000 && mode != 0x8000 && mode != 0x20000 && mode != 0x40000 && mode != 0x10000 {
        invalid_parameter();
        return EINVAL;
    }
    // SAFETY: the runtime's own static.
    unsafe { crate::_fmode = mode };
    0
}

/// `_get_fmode(&mode)`.
#[unsafe(no_mangle)]
pub extern "C" fn _get_fmode(mode: *mut i32) -> i32 {
    if mode.is_null() {
        invalid_parameter();
        return EINVAL;
    }
    // SAFETY: the caller's int; our static.
    unsafe { *mode = crate::_fmode };
    0
}

static NEW_MODE: AtomicI32 = AtomicI32::new(0);

/// `_set_new_mode(mode)`: whether malloc failures call the new handler;
/// the previous mode. (gala's malloc does not call it: recorded when
/// set.)
#[unsafe(no_mangle)]
pub extern "C" fn _set_new_mode(mode: i32) -> i32 {
    if mode != 0 && mode != 1 {
        invalid_parameter();
        return -1;
    }
    NEW_MODE.swap(mode, Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn _query_new_mode() -> i32 {
    NEW_MODE.load(Ordering::Relaxed)
}

/// `_callnewh(size)`: no new handler is ever installed here (C++'s
/// operator new installs it in the program's own runtime); 0.
#[unsafe(no_mangle)]
pub extern "C" fn _callnewh(_size: usize) -> i32 {
    0
}

/// `_set_error_mode(mode)`: where runtime errors are reported; the
/// previous mode (3: query).
#[unsafe(no_mangle)]
pub extern "C" fn _set_error_mode(mode: i32) -> i32 {
    static ERROR_MODE: AtomicI32 = AtomicI32::new(0);
    if mode == 3 {
        ERROR_MODE.load(Ordering::Relaxed)
    } else {
        ERROR_MODE.swap(mode, Ordering::Relaxed)
    }
}

// ---- The invalid-parameter handler and terminate --------------------------------------

/// The program's invalid-parameter handler
/// (`void (*)(const wchar_t*, const wchar_t*, const wchar_t*, unsigned, uintptr_t)`).
static INVALID_PARAMETER_HANDLER: AtomicUsize = AtomicUsize::new(0);

/// `_set_invalid_parameter_handler(h)`: the previous handler.
#[unsafe(no_mangle)]
pub extern "C" fn _set_invalid_parameter_handler(handler: usize) -> usize {
    INVALID_PARAMETER_HANDLER.swap(handler, Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn _get_invalid_parameter_handler() -> usize {
    INVALID_PARAMETER_HANDLER.load(Ordering::Relaxed)
}

/// What a C runtime function does with an invalid argument (the release
/// runtime's `_invalid_parameter_noinfo`): calls the thread's handler
/// (`_set_thread_local_invalid_parameter_handler`), else the program's,
/// with NULL expression, function and file, line 0, and returns; without
/// one the process ends with STATUS_STACK_BUFFER_OVERRUN (Windows'
/// fast-fail), after a line on stderr naming the cause.
pub(crate) fn invalid_parameter() {
    let h = match compat::thread_handler() {
        0 => INVALID_PARAMETER_HANDLER.load(Ordering::Relaxed),
        h => h,
    };
    if h != 0 {
        type Handler = extern "C" fn(*const u16, *const u16, *const u16, u32, usize);
        // SAFETY: registered by the program as such a handler.
        let h: Handler = unsafe { core::mem::transmute(h) };
        h(core::ptr::null(), core::ptr::null(), core::ptr::null(), 0, 0);
        return;
    }
    write_stderr(b"ucrtbase: invalid parameter passed to a C runtime function, ending the process (0xc0000409)\r\n");
    // SAFETY: ends this process.
    unsafe { TerminateProcess(GetCurrentProcess(), 0xc000_0409) };
}

#[unsafe(no_mangle)]
pub extern "C" fn _invalid_parameter_noinfo() {
    invalid_parameter();
}

/// `_invalid_parameter_noinfo_noreturn()`: the handler, then the process
/// ends whatever it did.
#[unsafe(no_mangle)]
pub extern "C" fn _invalid_parameter_noinfo_noreturn() -> ! {
    invalid_parameter();
    // SAFETY: ends this process.
    unsafe { TerminateProcess(GetCurrentProcess(), 0xc000_0409) };
    crate::_exit(0xc000_0409u32 as i32)
}

/// `_invalid_parameter(expression, function, file, line, reserved)`: the
/// debug runtime's form; the same here.
#[unsafe(no_mangle)]
pub extern "C" fn _invalid_parameter(
    _expression: *const u16,
    _function: *const u16,
    _file: *const u16,
    _line: u32,
    _reserved: usize,
) {
    invalid_parameter();
}

static TERMINATE_HANDLER: AtomicUsize = AtomicUsize::new(0);

/// `set_terminate(f)`: the previous terminate handler.
#[unsafe(no_mangle)]
pub extern "C" fn set_terminate(f: usize) -> usize {
    TERMINATE_HANDLER.swap(f, Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn _get_terminate() -> usize {
    TERMINATE_HANDLER.load(Ordering::Relaxed)
}

/// `terminate()`: the terminate handler if one is set, then abort.
#[unsafe(no_mangle)]
pub extern "C" fn terminate() -> ! {
    let f = TERMINATE_HANDLER.load(Ordering::Relaxed);
    if f != 0 {
        // SAFETY: registered as a `void (*)(void)`.
        let f: extern "C" fn() = unsafe { core::mem::transmute(f) };
        f();
    }
    crate::abort()
}

/// `_errno`'s companion for the Win32 error (`_doserrno`).
#[unsafe(no_mangle)]
pub extern "C" fn __doserrno() -> *mut u32 {
    static mut DOSERRNO: u32 = 0;
    &raw mut DOSERRNO
}

/// `_get_errno(&e)`.
#[unsafe(no_mangle)]
pub extern "C" fn _get_errno(e: *mut i32) -> i32 {
    if e.is_null() {
        invalid_parameter();
        return EINVAL;
    }
    // SAFETY: the caller's int; our errno.
    unsafe { *e = *crate::stdio::_errno() };
    0
}

/// `_set_errno(e)`.
#[unsafe(no_mangle)]
pub extern "C" fn _set_errno(e: i32) -> i32 {
    // SAFETY: our errno.
    unsafe { *crate::stdio::_errno() = e };
    0
}

/// malloc's companion that the UCRT exports for its own headers'
/// `_malloca`-style helpers: `_recalloc(p, count, size)`.
#[unsafe(no_mangle)]
pub extern "C" fn _recalloc(p: *mut u8, count: usize, size: usize) -> *mut u8 {
    let Some(total) = count.checked_mul(size) else {
        crate::wide::set_errno(12); // ENOMEM
        return null_mut();
    };
    let old = if p.is_null() { 0 } else { crate::heap::requested(p) };
    let q = realloc(p, total);
    if !q.is_null() && total > old {
        // SAFETY: the new bytes of the block.
        unsafe { core::ptr::write_bytes(q.add(old), 0, total - old) };
    }
    q
}

/// `_msize(p)`: the size the block was asked with.
#[unsafe(no_mangle)]
pub extern "C" fn _msize(p: *mut u8) -> usize {
    if p.is_null() {
        invalid_parameter();
        return usize::MAX;
    }
    crate::heap::requested(p)
}

/// `malloc` that the start-up code of `/MD` programs calls through
/// `_malloc_base` in debug-free builds.
#[unsafe(no_mangle)]
pub extern "C" fn _malloc_base(size: usize) -> *mut u8 {
    malloc(size)
}

#[unsafe(no_mangle)]
pub extern "C" fn _free_base(p: *mut u8) {
    free(p)
}
