//! gala's msvcrt.dll (ROADMAP N2a): the C runtime that mingw-w64 programs
//! (Nginx) import, written from the C standard and Microsoft's CRT
//! documentation. Only what programs have needed so far; anything else is
//! a missing export the loader reports to the ledger.
//!
//! Parts: CRT startup support (`__getmainargs`, `_initterm`, exit
//! handling), the heap (`heap.rs`), stdio to the console handles
//! (`stdio.rs`, with a printf engine), time conversion (`time.rs`),
//! strings, `qsort`, `rand`.
//!
//! Built for x64 (System32) and for x86 (SysWOW64's, ARCHITECTURE "Plan:
//! WOW64"): what differs by architecture is in `x64.rs` and `x86.rs` (the
//! variadic entry points, setjmp/longjmp, the floating-point control);
//! the `va_list` walker (`va.rs`) and the structures with pointer-sized
//! fields (FILE, lconv, STARTUPINFO, ...) take the architecture's sizes.

#![no_std]
#![allow(non_snake_case, non_upper_case_globals)]
// Every export is a C-ABI function called by foreign code with the raw
// pointers its documentation describes; Rust callers do not exist.
#![allow(clippy::not_unsafe_ptr_arg_deref)]
// Lints found when msvcrt joined CI's clippy line (2026-11-03); kept as
// written: `&mut *(&raw mut STATIC)` is the deliberate way to borrow a
// `static mut` without the static_mut_refs lint, and the C string, enum
// and fd-loop forms match the C runtime's layouts.
#![allow(
    clippy::deref_addrof,
    clippy::manual_c_str_literals,
    clippy::large_enum_variant,
    clippy::needless_range_loop
)]

// The architecture's parts: x64 (System32's msvcrt) or x86 (SysWOW64's).
#[cfg(target_arch = "x86_64")]
#[path = "x64.rs"]
mod arch;
#[cfg(target_arch = "x86")]
#[path = "x86.rs"]
mod arch;

mod float;
mod heap;
mod math;
mod misc;
mod posix;
mod scan;
mod stdio;
mod time;
mod va;
mod wide;
mod wide_more;

// gala's ucrtbase.dll: the same source with the UCRT's own functions
// (user/ucrtbase; ARCHITECTURE "The UCRT").
#[cfg(ucrt)]
mod ucrt;

// The 32-bit build: the memory routines, 64-bit division helpers and
// stack probe the compiler calls on x86 (and memcmp/strlen, exported as
// msvcrt's own) come from ntrt's x86 runtime.
#[cfg(target_arch = "x86")]
use ntrt as _;

use core::ffi::c_void;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, Ordering};

pub use heap::{calloc, free, malloc, realloc};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetCommandLineW() -> *const u16;
    fn GetEnvironmentStringsW() -> *mut u16;
    fn FreeEnvironmentStringsW(block: *mut u16) -> i32;
    fn ExitProcess(code: u32) -> !;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    // Named by core's unwind tables (MSVC target); ntdll has it.
    fn __CxxFrameHandler3();
}

// x86: ntrt's (above): strlen is a Rust function there, the memory
// routines are its assembly.
#[cfg(target_arch = "x86")]
use ntrt::strlen;
#[cfg(target_arch = "x86")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
}

/// Referenced by code that uses floating point (MSVC target convention).
/// On x86, ntrt's.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

/// STARTUPINFOA (winbase.h), its pointers and handles pointer-sized.
#[repr(C)]
pub(crate) struct StartupInfoA {
    pub cb: u32,
    pub reserved: usize,
    pub desktop: usize,
    pub title: usize,
    pub x: u32,
    pub y: u32,
    pub x_size: u32,
    pub y_size: u32,
    pub x_count_chars: u32,
    pub y_count_chars: u32,
    pub fill_attribute: u32,
    pub flags: u32,
    pub show_window: u16,
    pub cb_reserved2: u16,
    pub reserved2: *mut u8,
    pub std_input: usize,
    pub std_output: usize,
    pub std_error: usize,
}

const _: () =
    assert!(size_of::<StartupInfoA>() == if cfg!(target_arch = "x86") { 68 } else { 104 });

/// SECURITY_ATTRIBUTES (minwinbase.h).
#[repr(C)]
pub(crate) struct SecurityAttributes {
    length: u32,
    descriptor: usize,
    inherit: i32,
}

impl SecurityAttributes {
    /// No descriptor; the handle inheritable or not.
    pub(crate) fn inheritable(inherit: bool) -> Self {
        Self { length: size_of::<Self>() as u32, descriptor: 0, inherit: i32::from(inherit) }
    }
}

/// Reports a gap to the kernel's missing-features ledger.
fn missing(function: &[u8], detail: u64) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(function.as_ptr(), function.len(), 2, detail) };
}

/// A tiny spinlock for the runtime's shared state.
pub(crate) struct Lock(AtomicBool);

impl Lock {
    pub(crate) const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    pub(crate) fn with<R>(&self, f: impl FnOnce() -> R) -> R {
        while self
            .0
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        let r = f();
        self.0.store(false, Ordering::Release);
        r
    }
}

// ---- Startup ------------------------------------------------------------

/// Data exports the startup code reads and writes.
#[unsafe(no_mangle)]
pub static mut _environ: *mut *mut u8 = null_mut();
#[unsafe(no_mangle)]
pub static mut __initenv: *mut *mut u8 = null_mut();
#[unsafe(no_mangle)]
pub static mut _commode: i32 = 0;
#[unsafe(no_mangle)]
pub static mut _fmode: i32 = 0;

/// __p__fmode/__p__commode: the addresses of `_fmode` and `_commode`
/// (mingw-w64's startup code sets them through these).
#[unsafe(no_mangle)]
pub extern "C" fn __p__fmode() -> *mut i32 {
    &raw mut _fmode
}

#[unsafe(no_mangle)]
pub extern "C" fn __p__commode() -> *mut i32 {
    &raw mut _commode
}

/// _acmdln: the command line, narrowed (set when the DLL loads; MinGW's
/// startup code for GUI programs reads it).
#[unsafe(no_mangle)]
pub static mut _acmdln: *mut u8 = null_mut();
/// __argv: the program's arguments (set by __getmainargs).
#[unsafe(no_mangle)]
pub static mut __argv: *mut *mut u8 = null_mut();
/// _HUGE: HUGE_VAL for headers that read it from msvcrt (TinyCC's math.h).
#[unsafe(no_mangle)]
pub static _HUGE: f64 = f64::INFINITY;

/// __argc: their number (set by __getmainargs).
#[unsafe(no_mangle)]
pub static mut __argc: i32 = 0;

static mut ARGC: i32 = 0;
static mut ARGV: *mut *mut u8 = null_mut();
static STARTUP: Lock = Lock::new();

/// UTF-16 to a NUL-terminated UTF-8 string on the heap (the "ANSI" code
/// page programs see).
fn narrow(wide: &[u16]) -> *mut u8 {
    let mut len = 0;
    for c in char::decode_utf16(wide.iter().copied()) {
        len += c.map_or(3, char::len_utf8);
    }
    let out = malloc(len + 1);
    if out.is_null() {
        return out;
    }
    let mut at = 0;
    for c in char::decode_utf16(wide.iter().copied()) {
        let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
        let mut b = [0u8; 4];
        for &byte in c.encode_utf8(&mut b).as_bytes() {
            // SAFETY: `len` bytes were counted above.
            unsafe { *out.add(at) = byte };
            at += 1;
        }
    }
    // SAFETY: room for the terminator.
    unsafe { *out.add(at) = 0 };
    out
}

/// Splits a command line into arguments with the documented rules
/// ("Parsing C command-line arguments"): white space separates, double
/// quotes group, 2n backslashes before a quote become n (and the quote
/// toggles), 2n+1 become n and a literal quote; other backslashes are
/// literal. Calls `f` with each argument.
pub(crate) fn split_args(line: &[u16], mut f: impl FnMut(&[u16])) {
    const BS: u16 = b'\\' as u16;
    const QUOTE: u16 = b'"' as u16;
    let mut arg = [0u16; 1024];
    let mut i = 0;
    while i < line.len() {
        while i < line.len() && (line[i] == b' ' as u16 || line[i] == b'\t' as u16) {
            i += 1;
        }
        if i == line.len() {
            break;
        }
        let (mut n, mut quoted) = (0usize, false);
        while i < line.len() {
            let c = line[i];
            if !quoted && (c == b' ' as u16 || c == b'\t' as u16) {
                break;
            }
            if c == BS {
                let start = i;
                while i < line.len() && line[i] == BS {
                    i += 1;
                }
                let count = i - start;
                let before_quote = i < line.len() && line[i] == QUOTE;
                let keep = if before_quote { count / 2 } else { count };
                for _ in 0..keep {
                    if n < arg.len() {
                        arg[n] = BS;
                        n += 1;
                    }
                }
                if before_quote && count % 2 == 1 {
                    if n < arg.len() {
                        arg[n] = QUOTE;
                        n += 1;
                    }
                    i += 1;
                }
                continue;
            }
            if c == QUOTE {
                if quoted && i + 1 < line.len() && line[i + 1] == QUOTE {
                    // "" inside quotes: a literal quote.
                    if n < arg.len() {
                        arg[n] = QUOTE;
                        n += 1;
                    }
                    i += 2;
                    continue;
                }
                quoted = !quoted;
                i += 1;
                continue;
            }
            if n < arg.len() {
                arg[n] = c;
                n += 1;
            }
            i += 1;
        }
        f(&arg[..n]);
    }
}

fn build_args() {
    // SAFETY: the PEB's command line, NUL-terminated, for the process's
    // lifetime.
    let line = unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    };
    let mut count = 0usize;
    split_args(line, |_| count += 1);
    let argv = malloc((count + 1) * size_of::<usize>()).cast::<*mut u8>();
    if argv.is_null() {
        return;
    }
    let mut i = 0;
    split_args(line, |a| {
        // SAFETY: `count + 1` slots.
        unsafe { *argv.add(i) = narrow(a) };
        i += 1;
    });
    // SAFETY: as above; the runtime's own statics, set once under STARTUP.
    unsafe {
        *argv.add(count) = null_mut();
        ARGC = count as i32;
        ARGV = argv;
    }
}

fn build_environment() {
    // SAFETY: a copy of the environment block (k=v\0...\0\0) that we free.
    unsafe {
        let block = GetEnvironmentStringsW();
        if block.is_null() {
            return;
        }
        let mut entries = 0;
        let mut p = block;
        while *p != 0 {
            while *p != 0 {
                p = p.add(1);
            }
            entries += 1;
            p = p.add(1);
        }
        let env = malloc((entries + 1) * size_of::<usize>()).cast::<*mut u8>();
        if env.is_null() {
            FreeEnvironmentStringsW(block);
            return;
        }
        let (mut p, mut n) = (block, 0);
        while *p != 0 {
            let start = p;
            while *p != 0 {
                p = p.add(1);
            }
            let entry = core::slice::from_raw_parts(start, p.offset_from(start) as usize);
            // Per-drive current directories ("=C:=C:\") are not variables.
            if entry.first() != Some(&(b'=' as u16)) {
                *env.add(n) = narrow(entry);
                n += 1;
            }
            p = p.add(1);
        }
        *env.add(n) = null_mut();
        FreeEnvironmentStringsW(block);
        _environ = env;
        __initenv = env;
    }
}

/// __getmainargs(&argc, &argv, &envp, dowildcard, &startinfo): the
/// arguments and environment for main (built once).
#[unsafe(no_mangle)]
pub extern "C" fn __getmainargs(
    argc: *mut i32,
    argv: *mut *mut *mut u8,
    envp: *mut *mut *mut u8,
    _wildcard: i32,
    _startinfo: *mut c_void,
) -> i32 {
    STARTUP.with(|| {
        // SAFETY: the runtime's own statics.
        if unsafe { ARGV.is_null() } {
            build_args();
            build_environment();
        }
    });
    // SAFETY: the caller's output pointers; the statics are set.
    unsafe {
        *argc = ARGC;
        *argv = ARGV;
        __argv = ARGV;
        __argc = ARGC;
        *envp = _environ;
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn __set_app_type(_kind: i32) {}

#[unsafe(no_mangle)]
pub extern "C" fn __setusermatherr(_handler: *mut c_void) {}

/// _initterm(begin, end): calls each non-null function in the table.
#[unsafe(no_mangle)]
pub extern "C" fn _initterm(begin: *const usize, end: *const usize) {
    let mut p = begin;
    while p < end {
        // SAFETY: the table holds function pointers or zeros.
        unsafe {
            if *p != 0 {
                let f: extern "C" fn() = core::mem::transmute(*p);
                f();
            }
            p = p.add(1);
        }
    }
}

const MAX_EXIT_HANDLERS: usize = 64;
static mut EXIT_HANDLERS: [usize; MAX_EXIT_HANDLERS] = [0; MAX_EXIT_HANDLERS];
static mut EXIT_COUNT: usize = 0;
static EXIT_LOCK: Lock = Lock::new();

/// _onexit(f): registers `f` to run at exit; returns it (NULL if full).
#[unsafe(no_mangle)]
pub extern "C" fn _onexit(f: usize) -> usize {
    EXIT_LOCK.with(|| {
        // SAFETY: the table and count, under EXIT_LOCK.
        unsafe {
            if EXIT_COUNT == MAX_EXIT_HANDLERS {
                return 0;
            }
            (*(&raw mut EXIT_HANDLERS))[EXIT_COUNT] = f;
            EXIT_COUNT += 1;
        }
        f
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn atexit(f: usize) -> i32 {
    if _onexit(f) == 0 { -1 } else { 0 }
}

/// Runs the registered exit handlers, last registered first.
fn run_exit_handlers() {
    loop {
        let f = EXIT_LOCK.with(|| {
            // SAFETY: as in `_onexit`.
            unsafe {
                if EXIT_COUNT == 0 {
                    return 0;
                }
                EXIT_COUNT -= 1;
                (*(&raw const EXIT_HANDLERS))[EXIT_COUNT]
            }
        });
        if f == 0 {
            return;
        }
        // SAFETY: registered as a function pointer.
        let f: extern "C" fn() = unsafe { core::mem::transmute(f) };
        f();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _cexit() {
    #[cfg(ucrt)]
    ucrt::before_exit_handlers();
    run_exit_handlers();
}

#[unsafe(no_mangle)]
pub extern "C" fn exit(code: i32) -> ! {
    #[cfg(ucrt)]
    ucrt::before_exit_handlers();
    run_exit_handlers();
    // SAFETY: ends the process.
    unsafe { ExitProcess(code as u32) }
}

#[unsafe(no_mangle)]
pub extern "C" fn _exit(code: i32) -> ! {
    // SAFETY: ends the process without the exit handlers.
    unsafe { ExitProcess(code as u32) }
}

#[unsafe(no_mangle)]
pub extern "C" fn abort() -> ! {
    stdio::write_stderr(b"\r\nabnormal program termination\r\n");
    // SAFETY: ends the process (abort's exit code is 3).
    unsafe { ExitProcess(3) }
}

/// _fpreset(): the floating-point state programs start with (as Wine
/// documents msvcrt's): the x87 initialized with control word 0x27f (all
/// exceptions masked, 53-bit precision, round to nearest), MXCSR 0x1f80.
#[unsafe(no_mangle)]
pub extern "C" fn _fpreset() {
    let (cw, mx) = (0x27fu16, 0x1f80u32);
    // SAFETY: resets the x87 and loads the two documented control words.
    unsafe {
        core::arch::asm!(
            "fninit",
            "fldcw [{cw}]",
            "ldmxcsr [{mx}]",
            cw = in(reg) &raw const cw,
            mx = in(reg) &raw const mx,
            options(nostack),
        );
    }
}

/// _XcptFilter(code, pointers): the C runtime's exception filter, which
/// runs a signal() handler for the exception's signal. signal() installs
/// no handlers here, so the exception goes on to the next handler
/// (EXCEPTION_CONTINUE_SEARCH).
#[unsafe(no_mangle)]
pub extern "C" fn _XcptFilter(_code: u32, _pointers: *mut c_void) -> i32 {
    0
}

/// _assert(expression, file, line): reports a failed assert() on stderr
/// and aborts, as msvcrt's console programs do.
#[unsafe(no_mangle)]
pub extern "C" fn _assert(expression: *const u8, file: *const u8, line: u32) -> ! {
    let mut digits = [0u8; 10];
    let mut i = digits.len();
    let mut n = line;
    loop {
        i -= 1;
        digits[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    stdio::write_stderr(b"Assertion failed: ");
    // SAFETY: the caller's NUL-terminated strings.
    unsafe {
        stdio::write_stderr(c_str(expression));
        stdio::write_stderr(b", file ");
        stdio::write_stderr(c_str(file));
    }
    stdio::write_stderr(b", line ");
    stdio::write_stderr(&digits[i..]);
    stdio::write_stderr(b"\r\n");
    abort()
}

/// _amsg_exit(n): the runtime's fatal error "R60nn".
#[unsafe(no_mangle)]
pub extern "C" fn _amsg_exit(n: i32) -> ! {
    let msg = [
        b'r',
        b'u',
        b'n',
        b't',
        b'i',
        b'm',
        b'e',
        b' ',
        b'e',
        b'r',
        b'r',
        b'o',
        b'r',
        b' ',
        b'R',
        b'6',
        b'0',
        b'0' + (n / 10 % 10) as u8,
        b'0' + (n % 10) as u8,
        b'\r',
        b'\n',
    ];
    stdio::write_stderr(&msg);
    // SAFETY: ends the process.
    unsafe { ExitProcess(255) }
}

/// __C_specific_handler: the language handler of SEH __try blocks. gala
/// does not run __except filters yet: the exception keeps searching.
#[unsafe(no_mangle)]
pub extern "C" fn __C_specific_handler(
    _record: *mut c_void,
    _frame: usize,
    _context: *mut c_void,
    _dispatcher: *mut c_void,
) -> i32 {
    missing(b"msvcrt.dll!__C_specific_handler", 0);
    1 // ExceptionContinueSearch
}

/// signal(sig, handler): handlers are accepted but never called (gala has
/// no console control events yet); returns SIG_DFL.
#[unsafe(no_mangle)]
pub extern "C" fn signal(_sig: i32, _handler: usize) -> usize {
    0
}

/// setlocale(category, locale): only the "C" locale.
#[unsafe(no_mangle)]
pub extern "C" fn setlocale(_category: i32, _locale: *const u8) -> *const u8 {
    b"C\0".as_ptr()
}

/// getenv(name): case-insensitive, as on Windows. The narrow environment
/// is built on first use if the start-up code did not (a `wmain`
/// program's start-up builds only the wide one: the UCRT makes the other
/// when it is first needed).
#[unsafe(no_mangle)]
pub extern "C" fn getenv(name: *const u8) -> *mut u8 {
    STARTUP.with(|| {
        // SAFETY: the runtime's own statics, under STARTUP.
        if unsafe { _environ.is_null() } {
            build_environment();
        }
    });
    // SAFETY: the caller's NUL-terminated name; the environment array
    // built at startup.
    unsafe {
        if name.is_null() || _environ.is_null() {
            return null_mut();
        }
        let n = strlen(name);
        let name = core::slice::from_raw_parts(name, n);
        let mut p = _environ;
        while !(*p).is_null() {
            let entry = *p;
            let entry_len = strlen(entry);
            let e = core::slice::from_raw_parts(entry, entry_len);
            if entry_len > n && e[n] == b'=' && e[..n].eq_ignore_ascii_case(name) {
                return entry.add(n + 1);
            }
            p = p.add(1);
        }
        null_mut()
    }
}

// ---- Strings --------------------------------------------------------------

/// The bytes of a NUL-terminated string (without the terminator).
///
/// # Safety
/// `s` points to a NUL-terminated string.
unsafe fn c_str<'a>(s: *const u8) -> &'a [u8] {
    // SAFETY: forwarded.
    unsafe { core::slice::from_raw_parts(s, strlen(s)) }
}

#[unsafe(no_mangle)]
pub extern "C" fn memchr(s: *const u8, c: i32, n: usize) -> *mut u8 {
    let c = c as u8;
    for i in 0..n {
        // SAFETY: the caller's buffer of `n` bytes.
        unsafe {
            if *s.add(i) == c {
                return s.add(i).cast_mut();
            }
        }
    }
    null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn strchr(s: *const u8, c: i32) -> *mut u8 {
    let c = c as u8;
    let mut p = s;
    // SAFETY: the caller's NUL-terminated string (the terminator matches
    // a search for 0, as documented).
    unsafe {
        loop {
            if *p == c {
                return p.cast_mut();
            }
            if *p == 0 {
                return null_mut();
            }
            p = p.add(1);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn strcmp(a: *const u8, b: *const u8) -> i32 {
    strncmp(a, b, usize::MAX)
}

#[unsafe(no_mangle)]
pub extern "C" fn strncmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    // SAFETY: the caller's NUL-terminated strings.
    unsafe {
        for i in 0..n {
            let (x, y) = (*a.add(i), *b.add(i));
            if x != y {
                return i32::from(x) - i32::from(y);
            }
            if x == 0 {
                return 0;
            }
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn strpbrk(s: *const u8, accept: *const u8) -> *mut u8 {
    // SAFETY: the caller's NUL-terminated strings.
    unsafe {
        let accept = c_str(accept);
        let mut p = s;
        while *p != 0 {
            if accept.contains(&*p) {
                return p.cast_mut();
            }
            p = p.add(1);
        }
    }
    null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn strstr(haystack: *const u8, needle: *const u8) -> *mut u8 {
    // SAFETY: the caller's NUL-terminated strings.
    unsafe {
        let (h, n) = (c_str(haystack), c_str(needle));
        if n.is_empty() {
            return haystack.cast_mut();
        }
        match h.windows(n.len()).position(|w| w == n) {
            Some(i) => haystack.add(i).cast_mut(),
            None => null_mut(),
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tolower(c: i32) -> i32 {
    if (b'A' as i32..=b'Z' as i32).contains(&c) { c + 32 } else { c }
}

#[unsafe(no_mangle)]
pub extern "C" fn toupper(c: i32) -> i32 {
    if (b'a' as i32..=b'z' as i32).contains(&c) { c - 32 } else { c }
}

/// _wcsicmp: compares wide strings ignoring ASCII case.
#[unsafe(no_mangle)]
pub extern "C" fn _wcsicmp(a: *const u16, b: *const u16) -> i32 {
    let fold = |c: u16| if (b'A' as u16..=b'Z' as u16).contains(&c) { c + 32 } else { c };
    // SAFETY: the caller's NUL-terminated wide strings.
    unsafe {
        let mut i = 0;
        loop {
            let (x, y) = (fold(*a.add(i)), fold(*b.add(i)));
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

// ---- qsort, rand -------------------------------------------------------------

/// qsort(base, count, size, compare): heapsort (in place, no allocation).
#[unsafe(no_mangle)]
pub extern "C" fn qsort(
    base: *mut u8,
    count: usize,
    size: usize,
    compare: extern "C" fn(*const u8, *const u8) -> i32,
) {
    if count < 2 || size == 0 {
        return;
    }
    let at = |i: usize| -> *mut u8 {
        // SAFETY: i < count, inside the caller's array.
        unsafe { base.add(i * size) }
    };
    let swap = |i: usize, j: usize| {
        // SAFETY: two distinct elements of the array.
        unsafe { core::ptr::swap_nonoverlapping(at(i), at(j), size) };
    };
    let sift_down = |mut root: usize, end: usize| loop {
        let mut child = 2 * root + 1;
        if child >= end {
            return;
        }
        if child + 1 < end && compare(at(child), at(child + 1)) < 0 {
            child += 1;
        }
        if compare(at(root), at(child)) >= 0 {
            return;
        }
        swap(root, child);
        root = child;
    };
    for start in (0..count / 2).rev() {
        sift_down(start, count);
    }
    for end in (1..count).rev() {
        swap(0, end);
        sift_down(0, end);
    }
}

static mut RAND_STATE: u32 = 1;

/// rand: the documented generator (seed * 214013 + 2531011, bits 16-30).
#[unsafe(no_mangle)]
pub extern "C" fn rand() -> i32 {
    // SAFETY: a plain integer; MS keeps it per thread, gala per process.
    unsafe {
        RAND_STATE = RAND_STATE.wrapping_mul(214_013).wrapping_add(2_531_011);
        ((RAND_STATE >> 16) & 0x7fff) as i32
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn srand(seed: u32) {
    // SAFETY: as in `rand`.
    unsafe { RAND_STATE = seed };
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, reason: u32, _reserved: usize) -> i32 {
    if reason == 1 {
        // DLL_PROCESS_ATTACH: descriptors inherited from a parent's CRT.
        stdio::inherit_descriptors();
        // SAFETY: the PEB's command line, NUL-terminated, for the
        // process's lifetime; the static is set once, before the program
        // runs.
        unsafe {
            let p = GetCommandLineW();
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            _acmdln = narrow(core::slice::from_raw_parts(p, n));
        }
    }
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    abort()
}
