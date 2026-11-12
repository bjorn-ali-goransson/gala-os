//! UCRT functions that the scenarios' programs (CPython, ...) import and
//! msvcrt's source lacks: the console I/O `_getwch` family, the wide
//! spawn functions, `_wsystem`, the `64i32` stat forms, `_wputenv_s`,
//! the time-zone variables, `__sys_errlist`, `_locking`, and the
//! per-thread invalid-parameter handler.

use super::{EINVAL, invalid_parameter};
use crate::heap::{free, malloc};
use crate::wide::{set_errno, wlen};
use core::ffi::c_void;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn TlsAlloc() -> u32;
    fn TlsGetValue(index: u32) -> *mut c_void;
    fn TlsSetValue(index: u32, value: *mut c_void) -> i32;
    fn GetLastError() -> u32;
    fn SetLastError(error: u32);
    fn LockFile(h: *mut c_void, low: u32, high: u32, len_low: u32, len_high: u32) -> i32;
    fn UnlockFile(h: *mut c_void, low: u32, high: u32, len_low: u32, len_high: u32) -> i32;
    fn SetFilePointerEx(h: *mut c_void, distance: i64, new: *mut i64, method: u32) -> i32;
    fn Sleep(ms: u32);
}

// ---- Console I/O --------------------------------------------------------------

/// The character `_ungetch`/`_ungetwch` pushed back (-1: none).
static PUSHED_BACK: AtomicI32 = AtomicI32::new(-1);

/// `_getwch()`: the next console character (a pushed-back one first).
#[unsafe(no_mangle)]
pub extern "C" fn _getwch() -> u16 {
    let c = PUSHED_BACK.swap(-1, Ordering::Relaxed);
    if c >= 0 {
        return c as u16;
    }
    let b = crate::posix::_getch();
    if b < 0 { 0xffff } else { b as u16 }
}

/// `_getwche()`: `_getwch` echoed.
#[unsafe(no_mangle)]
pub extern "C" fn _getwche() -> u16 {
    let c = _getwch();
    if c != 0xffff {
        _putwch(c);
    }
    c
}

/// `_ungetch(c)` / `_ungetwch(c)`: one character back; EOF if one is
/// already there.
#[unsafe(no_mangle)]
pub extern "C" fn _ungetch(c: i32) -> i32 {
    match PUSHED_BACK.compare_exchange(-1, c & 0xffff, Ordering::Relaxed, Ordering::Relaxed) {
        Ok(_) => c,
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _ungetwch(c: u16) -> u16 {
    if _ungetch(i32::from(c)) < 0 { 0xffff } else { c }
}

/// `_putch(c)`: a byte to the console.
#[unsafe(no_mangle)]
pub extern "C" fn _putch(c: i32) -> i32 {
    crate::stdio::write_console(&[c as u8]);
    c
}

/// `_putwch(c)`: a character to the console (UTF-8).
#[unsafe(no_mangle)]
pub extern "C" fn _putwch(c: u16) -> u16 {
    let ch = char::from_u32(c.into()).unwrap_or(char::REPLACEMENT_CHARACTER);
    let mut b = [0u8; 4];
    crate::stdio::write_console(ch.encode_utf8(&mut b).as_bytes());
    c
}

/// `_kbhit()`: whether a key waits. gala's console input is a stream of
/// bytes, not key events: none is reported waiting (recorded).
#[unsafe(no_mangle)]
pub extern "C" fn _kbhit() -> i32 {
    crate::missing(b"ucrtbase.dll!_kbhit", 0);
    0
}

// ---- Processes ------------------------------------------------------------------

/// A wide string as a heap UTF-8 copy (NULL for NULL or no memory).
fn narrow_copy(w: *const u16) -> *mut u8 {
    if w.is_null() {
        return null_mut();
    }
    // SAFETY: the caller's NUL-terminated string.
    let units = unsafe { core::slice::from_raw_parts(w, wlen(w)) };
    let out = malloc(3 * units.len() + 1);
    if out.is_null() {
        return out;
    }
    let mut n = 0;
    for c in char::decode_utf16(units.iter().copied()) {
        let mut b = [0u8; 4];
        for &x in c.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut b).as_bytes() {
            // SAFETY: at most 3 bytes per unit.
            unsafe { *out.add(n) = x };
            n += 1;
        }
    }
    // SAFETY: room for the NUL.
    unsafe { *out.add(n) = 0 };
    out
}

/// A NULL-terminated vector of wide strings as heap UTF-8 copies (NULL
/// stays NULL); `None` without memory.
fn narrow_vector(v: *const *const u16) -> Option<*mut *const u8> {
    if v.is_null() {
        return Some(null_mut());
    }
    let mut count = 0;
    // SAFETY: the caller's NULL-terminated vector.
    while !unsafe { *v.add(count) }.is_null() {
        count += 1;
    }
    let out = malloc((count + 1) * size_of::<usize>()).cast::<*const u8>();
    if out.is_null() {
        return None;
    }
    for i in 0..count {
        // SAFETY: inside both vectors.
        unsafe {
            let s = narrow_copy(*v.add(i));
            *out.add(i) = s;
            if s.is_null() {
                *out.add(i + 1) = core::ptr::null();
                free_vector(out);
                return None;
            }
        }
    }
    // SAFETY: the terminator's slot.
    unsafe { *out.add(count) = core::ptr::null() };
    Some(out)
}

fn free_vector(v: *mut *const u8) {
    if v.is_null() {
        return;
    }
    let mut i = 0;
    // SAFETY: our NULL-terminated vector of heap strings.
    unsafe {
        while !(*v.add(i)).is_null() {
            free((*v.add(i)).cast_mut());
            i += 1;
        }
    }
    free(v.cast());
}

/// `_wspawnve(mode, cmdname, argv, envp)`: `_spawnve` with the strings
/// in UTF-8 (gala's ANSI code page, so nothing is lost).
#[unsafe(no_mangle)]
pub extern "C" fn _wspawnve(
    mode: i32,
    cmdname: *const u16,
    argv: *const *const u16,
    envp: *const *const u16,
) -> isize {
    let name = narrow_copy(cmdname);
    let (Some(args), Some(env)) = (narrow_vector(argv), narrow_vector(envp)) else {
        free(name);
        set_errno(12); // ENOMEM
        return -1;
    };
    let r = crate::posix::_spawnve(mode, name, args.cast_const(), env.cast_const());
    free(name);
    free_vector(args);
    free_vector(env);
    r
}

#[unsafe(no_mangle)]
pub extern "C" fn _wspawnv(mode: i32, cmdname: *const u16, argv: *const *const u16) -> isize {
    _wspawnve(mode, cmdname, argv, core::ptr::null())
}

/// `_wexecve(path, argv, envp)`: starts the program and ends this process
/// (as gala's `_execv`).
#[unsafe(no_mangle)]
pub extern "C" fn _wexecve(
    path: *const u16,
    argv: *const *const u16,
    envp: *const *const u16,
) -> isize {
    if _wspawnve(1, path, argv, envp) == -1 {
        return -1;
    }
    crate::_exit(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn _wexecv(path: *const u16, argv: *const *const u16) -> isize {
    _wexecve(path, argv, core::ptr::null())
}

/// `_wsystem(command)`: `system` with the command in UTF-8.
#[unsafe(no_mangle)]
pub extern "C" fn _wsystem(command: *const u16) -> i32 {
    if command.is_null() {
        return crate::posix::system(core::ptr::null());
    }
    let c = narrow_copy(command);
    if c.is_null() {
        set_errno(12);
        return -1;
    }
    let r = crate::posix::system(c);
    free(c);
    r
}

// ---- Files ----------------------------------------------------------------------------

/// `_stat64i32` and its forms: `struct _stat64i32` has `_stat64`'s layout
/// up to the size, which is 32 bits followed by padding where `_stat64`'s
/// is 64 (both 56 bytes; the times at 32, 40 and 48), so `_stat64`'s
/// result is also a `_stat64i32` (the size's low half).
#[unsafe(no_mangle)]
pub extern "C" fn _stat64i32(path: *const u8, buf: *mut u8) -> i32 {
    crate::stdio::_stat64(path, buf)
}

#[unsafe(no_mangle)]
pub extern "C" fn _wstat64i32(path: *const u16, buf: *mut u8) -> i32 {
    crate::wide_more::_wstat64(path, buf)
}

#[unsafe(no_mangle)]
pub extern "C" fn _fstat64i32(fd: i32, buf: *mut u8) -> i32 {
    crate::stdio::_fstat64(fd, buf)
}

/// `_locking(fd, mode, nbytes)`: locks or unlocks `nbytes` from the
/// current position: _LK_UNLCK 0, _LK_LOCK 1 and _LK_RLCK 3 (retried
/// once a second, ten times), _LK_NBLCK 2 and _LK_NBRLCK 4 (once).
#[unsafe(no_mangle)]
pub extern "C" fn _locking(fd: i32, mode: i32, nbytes: i32) -> i32 {
    let h = crate::stdio::_get_osfhandle(fd);
    if h == -1 || nbytes < 0 || !(0..=4).contains(&mode) {
        invalid_parameter();
        set_errno(EINVAL);
        return -1;
    }
    let h = h as *mut c_void;
    let mut at = 0i64;
    // SAFETY: our descriptor's handle.
    unsafe { SetFilePointerEx(h, 0, &mut at, 1) };
    let (low, high) = (at as u32, (at >> 32) as u32);
    // SAFETY: as above.
    let ok = unsafe {
        if mode == 0 {
            UnlockFile(h, low, high, nbytes as u32, 0) != 0
        } else {
            let tries = if mode == 1 || mode == 3 { 10 } else { 1 };
            let mut ok = false;
            for i in 0..tries {
                if LockFile(h, low, high, nbytes as u32, 0) != 0 {
                    ok = true;
                    break;
                }
                if i + 1 < tries {
                    Sleep(1000);
                }
            }
            ok
        }
    };
    if ok {
        0
    } else {
        set_errno(if mode == 0 { 13 } else { 36 }); // EACCES, EDEADLOCK
        -1
    }
}

/// `_heapmin()`: gives unused heap back; gala's heap keeps its regions: 0.
#[unsafe(no_mangle)]
pub extern "C" fn _heapmin() -> i32 {
    0
}

// ---- Environment ---------------------------------------------------------------

/// `_wputenv_s(name, value)`: `_wputenv("name=value")`.
#[unsafe(no_mangle)]
pub extern "C" fn _wputenv_s(name: *const u16, value: *const u16) -> i32 {
    if name.is_null() || value.is_null() {
        invalid_parameter();
        return EINVAL;
    }
    // SAFETY: the caller's NUL-terminated strings.
    let (n, v) = unsafe { (wlen(name), wlen(value)) };
    let entry = malloc(2 * (n + v + 2)).cast::<u16>();
    if entry.is_null() {
        return 12; // ENOMEM
    }
    // SAFETY: room for name, '=', value and the NUL.
    unsafe {
        core::ptr::copy_nonoverlapping(name, entry, n);
        *entry.add(n) = u16::from(b'=');
        core::ptr::copy_nonoverlapping(value, entry.add(n + 1), v);
        *entry.add(n + 1 + v) = 0;
    }
    let r = crate::wide_more::_wputenv(entry);
    free(entry.cast());
    if r == 0 { 0 } else { EINVAL }
}

/// `_putenv_s(name, value)`.
#[unsafe(no_mangle)]
pub extern "C" fn _putenv_s(name: *const u8, value: *const u8) -> i32 {
    if name.is_null() || value.is_null() {
        invalid_parameter();
        return EINVAL;
    }
    // SAFETY: the caller's NUL-terminated strings.
    let (n, v) = unsafe { (crate::c_str(name), crate::c_str(value)) };
    let entry = malloc(n.len() + v.len() + 2);
    if entry.is_null() {
        return 12;
    }
    // SAFETY: room for name, '=', value and the NUL.
    unsafe {
        core::ptr::copy_nonoverlapping(n.as_ptr(), entry, n.len());
        *entry.add(n.len()) = b'=';
        core::ptr::copy_nonoverlapping(v.as_ptr(), entry.add(n.len() + 1), v.len());
        *entry.add(n.len() + 1 + v.len()) = 0;
    }
    let r = crate::posix::_putenv(entry);
    free(entry);
    if r == 0 { 0 } else { EINVAL }
}

// ---- Time zone, error texts, abort behaviour -------------------------------------

/// gala's local time is UTC: no offset, no daylight saving.
static mut TIMEZONE: i32 = 0;
static mut DAYLIGHT: i32 = 0;
static mut DSTBIAS: i32 = 0;
static mut TZNAME: [*const u8; 2] = [b"UTC\0".as_ptr(), b"UTC\0".as_ptr()];

#[unsafe(no_mangle)]
pub extern "C" fn __timezone() -> *mut i32 {
    &raw mut TIMEZONE
}

#[unsafe(no_mangle)]
pub extern "C" fn __daylight() -> *mut i32 {
    &raw mut DAYLIGHT
}

#[unsafe(no_mangle)]
pub extern "C" fn __dstbias() -> *mut i32 {
    &raw mut DSTBIAS
}

#[unsafe(no_mangle)]
pub extern "C" fn __tzname() -> *mut *const u8 {
    (&raw mut TZNAME).cast()
}

/// `_get_timezone(&seconds)`, `_get_daylight(&hours)`.
#[unsafe(no_mangle)]
pub extern "C" fn _get_timezone(seconds: *mut i32) -> i32 {
    if seconds.is_null() {
        invalid_parameter();
        return EINVAL;
    }
    // SAFETY: the caller's long.
    unsafe { *seconds = 0 };
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn _get_daylight(hours: *mut i32) -> i32 {
    if hours.is_null() {
        invalid_parameter();
        return EINVAL;
    }
    // SAFETY: the caller's int.
    unsafe { *hours = 0 };
    0
}

/// `__sys_errlist()` / `__sys_nerr()`: `strerror`'s texts by number.
static mut ERRLIST: [*const u8; 44] = [core::ptr::null(); 44];
static ERRLIST_BUILT: AtomicU32 = AtomicU32::new(0);
static ERRLIST_LOCK: crate::Lock = crate::Lock::new();

#[unsafe(no_mangle)]
pub extern "C" fn __sys_errlist() -> *const *const u8 {
    ERRLIST_LOCK.with(|| {
        if ERRLIST_BUILT.load(Ordering::Relaxed) == 0 {
            for i in 0..44 {
                // SAFETY: our static, filled once under the lock.
                unsafe { (*(&raw mut ERRLIST))[i] = crate::misc::strerror(i as i32) };
            }
            ERRLIST_BUILT.store(1, Ordering::Relaxed);
        }
    });
    (&raw const ERRLIST).cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn __sys_nerr() -> *const i32 {
    static NERR: i32 = 43;
    &NERR
}

static ABORT_BEHAVIOR: AtomicU32 = AtomicU32::new(1); // _WRITE_ABORT_MSG

/// `_set_abort_behavior(flags, mask)`: the previous flags (`abort` writes
/// its message and ends the process with 3 either way here).
#[unsafe(no_mangle)]
pub extern "C" fn _set_abort_behavior(flags: u32, mask: u32) -> u32 {
    let old = ABORT_BEHAVIOR.load(Ordering::Relaxed);
    ABORT_BEHAVIOR.store((old & !mask) | (flags & mask), Ordering::Relaxed);
    old
}

// ---- The per-thread invalid-parameter handler -------------------------------------

/// The TLS slot of the per-thread handler (allocated on first use; 0:
/// none yet, u32::MAX: none could be had).
static TLS_SLOT: AtomicU32 = AtomicU32::new(0);
static TLS_LOCK: crate::Lock = crate::Lock::new();

fn slot() -> Option<u32> {
    let s = TLS_SLOT.load(Ordering::Acquire);
    if s != 0 {
        return (s != u32::MAX).then_some(s - 1);
    }
    TLS_LOCK.with(|| {
        if TLS_SLOT.load(Ordering::Acquire) == 0 {
            // SAFETY: plain call.
            let i = unsafe { TlsAlloc() };
            TLS_SLOT.store(if i == u32::MAX { u32::MAX } else { i + 1 }, Ordering::Release);
        }
    });
    let s = TLS_SLOT.load(Ordering::Acquire);
    (s != u32::MAX).then_some(s - 1)
}

/// This thread's handler (0: none). The thread's last error stays as it
/// was (TlsGetValue clears it, as Windows' does; the UCRT's per-thread
/// data does not touch it: CPython sets and restores the handler around
/// a failing call and reads the error after).
pub(crate) fn thread_handler() -> usize {
    let s = TLS_SLOT.load(Ordering::Acquire);
    if s == 0 || s == u32::MAX {
        return 0;
    }
    // SAFETY: our slot; plain calls.
    unsafe {
        let error = GetLastError();
        let h = TlsGetValue(s - 1) as usize;
        SetLastError(error);
        h
    }
}

/// `_set_thread_local_invalid_parameter_handler(h)`: this thread's
/// handler, which comes before the process's; the previous one.
#[unsafe(no_mangle)]
pub extern "C" fn _set_thread_local_invalid_parameter_handler(h: usize) -> usize {
    let Some(s) = slot() else {
        crate::missing(b"ucrtbase.dll!_set_thread_local_invalid_parameter_handler(TlsAlloc)", 0);
        return 0;
    };
    let old = thread_handler();
    // SAFETY: our slot; plain calls (the last error kept, as above).
    unsafe {
        let error = GetLastError();
        TlsSetValue(s, h as *mut c_void);
        SetLastError(error);
    }
    old
}

#[unsafe(no_mangle)]
pub extern "C" fn _get_thread_local_invalid_parameter_handler() -> usize {
    thread_handler()
}

/// `fma(x, y, z)`: x * y + z, rounded once.
#[unsafe(no_mangle)]
pub extern "C" fn fma(x: f64, y: f64, z: f64) -> f64 {
    libm::fma(x, y, z)
}

#[unsafe(no_mangle)]
pub extern "C" fn fmaf(x: f32, y: f32, z: f32) -> f32 {
    libm::fmaf(x, y, z)
}

/// `_sopen(path, flags, share, mode)`: `_open` (share modes are not
/// applied: gala opens files sharing read and write). The mode is a
/// variadic argument read only with _O_CREAT; an int in the fourth slot
/// either way on both architectures' conventions.
#[unsafe(no_mangle)]
pub extern "C" fn _sopen(path: *const u8, flags: i32, _share: i32, mode: i32) -> i32 {
    crate::stdio::_open(path, flags, mode)
}

/// `__conio_common_vcprintf(options, format, locale, args)`: printf to
/// the console (`_cprintf`): standard output's handle here.
#[unsafe(no_mangle)]
pub extern "C" fn __conio_common_vcprintf(
    options: u64,
    fmt: *const u8,
    locale: usize,
    args: crate::va::VaList,
) -> i32 {
    super::stdio::__stdio_common_vfprintf(
        options,
        crate::stdio::__iob_func().wrapping_add(crate::stdio::FILE_SIZE),
        fmt,
        locale,
        args,
    )
}
