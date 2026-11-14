//! Named events and mutexes (the object names Win32 programs pass live in
//! the caller's session's BaseNamedObjects: `\BaseNamedObjects` in session
//! 0, `\Sessions\<n>\BaseNamedObjects` in the others, where `Global\` and
//! `Local\` are the kernel's links to the global and the session's
//! directory; nt::session), and critical sections.

use super::*;
use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn NtOpenEvent(handle: *mut Handle, access: u32, attributes: *const ObjectAttributes) -> u32;
    fn NtClearEvent(h: Handle) -> u32;
    fn NtCreateMutant(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
        owner: u8,
    ) -> u32;
    fn NtOpenMutant(handle: *mut Handle, access: u32, attributes: *const ObjectAttributes) -> u32;
    fn NtOpenSemaphore(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
    ) -> u32;
    fn NtReleaseMutant(h: Handle, previous: *mut i32) -> u32;
}

const OBJ_OPENIF: u32 = 0x80;
const EVENT_ALL_ACCESS: u32 = 0x1f_0003;
const MUTANT_ALL_ACCESS: u32 = 0x1f_0001;

/// Converts a NUL-terminated ANSI (UTF-8) string to UTF-16 in `out`;
/// returns the length, or None if it does not fit.
pub(crate) fn ansi_to_wide(s: *const u8, out: &mut [u16]) -> Option<usize> {
    // SAFETY: the caller's NUL-terminated string.
    let bytes = unsafe { core::slice::from_raw_parts(s, strlen(s)) };
    let text = core::str::from_utf8(bytes).ok()?;
    let mut n = 0;
    for c in text.encode_utf16() {
        *out.get_mut(n)? = c;
        n += 1;
    }
    *out.get_mut(n)? = 0;
    Some(n)
}

/// PEB.SessionId: the caller's session.
pub(crate) fn session_id() -> u32 {
    // SAFETY: the TEB's PEB; its SessionId.
    unsafe { *((teb_read(TEB_PEB) + PEB_SESSION_ID) as *const u32) }
}

/// Writes `<the session's BaseNamedObjects>\<name>` (UTF-16, `name`
/// NUL-terminated) into `path`; returns its length, or None if it does
/// not fit. Session 0's directory is `\BaseNamedObjects`, another
/// session's `\Sessions\<n>\BaseNamedObjects`, as Windows' kernel32
/// names them.
pub(crate) fn session_path(name: *const u16, path: &mut [u16]) -> Option<usize> {
    let mut n = 0;
    let mut put = |c: u16, n: &mut usize| -> Option<()> {
        *path.get_mut(*n)? = c;
        *n += 1;
        Some(())
    };
    let session = session_id();
    if session != 0 {
        for c in "\\Sessions\\".encode_utf16() {
            put(c, &mut n)?;
        }
        let mut digits = [0u16; 10];
        let (mut k, mut v) = (0, session);
        loop {
            digits[k] = u16::from(b'0') + (v % 10) as u16;
            k += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        for &d in digits[..k].iter().rev() {
            put(d, &mut n)?;
        }
    }
    for c in "\\BaseNamedObjects\\".encode_utf16().chain(wide_slice(name).iter().copied()) {
        put(c, &mut n)?;
    }
    Some(n)
}

/// Whether SECURITY_ATTRIBUTES (nLength, lpSecurityDescriptor,
/// bInheritHandle) ask for an inheritable handle.
pub(crate) fn inheritable(attributes: *mut c_void) -> bool {
    // SAFETY: the caller's SECURITY_ATTRIBUTES (or NULL).
    !attributes.is_null() && unsafe { (*attributes.cast::<SecurityAttributes>()).inherit } != 0
}

const OBJ_INHERIT: u32 = 0x2;

/// Runs `f` with OBJECT_ATTRIBUTES naming `name` (UTF-16, NUL-terminated)
/// in the session's object directory (none for a null name), with
/// OBJ_INHERIT if `inherit`.
pub(crate) fn with_name<R>(
    name: *const u16,
    inherit: bool,
    f: impl FnOnce(*const ObjectAttributes) -> R,
) -> Result<R, u32> {
    with_name_and_descriptor(name, inherit, 0, f)
}

/// with_name with SECURITY_ATTRIBUTES (descriptor and inheritance).
pub(crate) fn with_name_sa<R>(
    name: *const u16,
    sa: *mut c_void,
    f: impl FnOnce(*const ObjectAttributes) -> R,
) -> Result<R, u32> {
    with_name_and_descriptor(name, inheritable(sa), super::security_descriptor(sa), f)
}

fn with_name_and_descriptor<R>(
    name: *const u16,
    inherit: bool,
    descriptor: usize,
    f: impl FnOnce(*const ObjectAttributes) -> R,
) -> Result<R, u32> {
    let inherit_bit = if inherit { OBJ_INHERIT } else { 0 };
    if name.is_null() {
        if !inherit && descriptor == 0 {
            return Ok(f(core::ptr::null()));
        }
        let oa = ObjectAttributes {
            length: size_of::<ObjectAttributes>() as u32,
            root_directory: 0,
            object_name: core::ptr::null(),
            attributes: inherit_bit,
            security_descriptor: descriptor,
            security_quality_of_service: 0,
        };
        return Ok(f(&oa));
    }
    let mut path = [0u16; 300];
    let n = session_path(name, &mut path).ok_or(ERROR_INVALID_PARAMETER)?;
    let us = UnicodeString {
        length: (n * 2) as u16,
        maximum_length: (n * 2) as u16,
        buffer: path.as_ptr(),
    };
    let oa = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root_directory: 0,
        object_name: &us,
        attributes: OBJ_OPENIF | inherit_bit,
        security_descriptor: descriptor,
        security_quality_of_service: 0,
    };
    Ok(f(&oa))
}

/// An ANSI name as UTF-16 (None: no name); Err: ERROR_INVALID_PARAMETER.
pub(crate) fn wide_name(name: *const u8, buf: &mut [u16; 260]) -> Result<*const u16, u32> {
    if name.is_null() {
        return Ok(core::ptr::null());
    }
    ansi_to_wide(name, buf).ok_or(ERROR_INVALID_PARAMETER)?;
    Ok(buf.as_ptr())
}

/// The result of creating a possibly named object: the handle, with
/// ERROR_ALREADY_EXISTS when an existing one was opened.
pub(crate) fn created(status: u32, h: Handle) -> Handle {
    match status {
        STATUS_SUCCESS => {
            SetLastError(0);
            h
        }
        STATUS_OBJECT_NAME_EXISTS => fail_with(ERROR_ALREADY_EXISTS, h),
        _ => fail(status, core::ptr::null_mut()),
    }
}

/// CreateEventW: optionally named (opening an existing event).
#[unsafe(no_mangle)]
pub extern "system" fn CreateEventW(
    attributes: *mut c_void,
    manual_reset: Bool,
    initial: Bool,
    name: *const u16,
) -> Handle {
    let mut h = core::ptr::null_mut();
    // NotificationEvent (0) for manual reset, else SynchronizationEvent (1).
    let kind = if manual_reset != 0 { 0 } else { 1 };
    let status = with_name_sa(name, attributes, |oa| {
        // SAFETY: valid output pointer and attributes.
        unsafe { NtCreateEvent(&mut h, EVENT_ALL_ACCESS, oa.cast(), kind, u8::from(initial != 0)) }
    });
    match status {
        Ok(status) => created(status, h),
        Err(e) => fail_with(e, core::ptr::null_mut()),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateEventA(
    attributes: *mut c_void,
    manual_reset: Bool,
    initial: Bool,
    name: *const u8,
) -> Handle {
    let mut buf = [0u16; 260];
    match wide_name(name, &mut buf) {
        Ok(name) => CreateEventW(attributes, manual_reset, initial, name),
        Err(e) => fail_with(e, core::ptr::null_mut()),
    }
}

fn open_named(
    name: *const u16,
    access: u32,
    open: unsafe extern "system" fn(*mut Handle, u32, *const ObjectAttributes) -> u32,
) -> Handle {
    if name.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, core::ptr::null_mut());
    }
    let mut h = core::ptr::null_mut();
    // SAFETY: valid output pointer and attributes.
    match with_name(name, false, |oa| unsafe { open(&mut h, access, oa) }) {
        Ok(STATUS_SUCCESS) => h,
        Ok(status) => fail(status, core::ptr::null_mut()),
        Err(e) => fail_with(e, core::ptr::null_mut()),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn OpenEventW(access: u32, _inherit: Bool, name: *const u16) -> Handle {
    open_named(name, access, NtOpenEvent)
}

#[unsafe(no_mangle)]
pub extern "system" fn OpenEventA(access: u32, inherit: Bool, name: *const u8) -> Handle {
    let mut buf = [0u16; 260];
    match wide_name(name, &mut buf) {
        Ok(name) => OpenEventW(access, inherit, name),
        Err(e) => fail_with(e, core::ptr::null_mut()),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn ResetEvent(h: Handle) -> Bool {
    // SAFETY: the kernel validates the handle.
    let status = unsafe { NtClearEvent(h) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateMutexW(
    attributes: *mut c_void,
    owner: Bool,
    name: *const u16,
) -> Handle {
    let mut h = core::ptr::null_mut();
    let status = with_name_sa(name, attributes, |oa| {
        // SAFETY: valid output pointer and attributes.
        unsafe { NtCreateMutant(&mut h, MUTANT_ALL_ACCESS, oa, u8::from(owner != 0)) }
    });
    match status {
        Ok(status) => created(status, h),
        Err(e) => fail_with(e, core::ptr::null_mut()),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateMutexA(
    attributes: *mut c_void,
    owner: Bool,
    name: *const u8,
) -> Handle {
    let mut buf = [0u16; 260];
    match wide_name(name, &mut buf) {
        Ok(name) => CreateMutexW(attributes, owner, name),
        Err(e) => fail_with(e, core::ptr::null_mut()),
    }
}

/// CreateMutexExW(attributes, name, flags, access): CreateMutexW with the
/// access asked for; CREATE_MUTEX_INITIAL_OWNER (1) is the only flag.
#[unsafe(no_mangle)]
pub extern "system" fn CreateMutexExW(
    attributes: *mut c_void,
    name: *const u16,
    flags: u32,
    access: u32,
) -> Handle {
    if flags & !1 != 0 {
        return fail_with(ERROR_INVALID_PARAMETER, core::ptr::null_mut());
    }
    let mut h = core::ptr::null_mut();
    let status = with_name_sa(name, attributes, |oa| {
        // SAFETY: valid output pointer and attributes.
        unsafe { NtCreateMutant(&mut h, access, oa, u8::from(flags & 1 != 0)) }
    });
    match status {
        Ok(status) => created(status, h),
        Err(e) => fail_with(e, core::ptr::null_mut()),
    }
}

/// CreateMutexExA.
#[unsafe(no_mangle)]
pub extern "system" fn CreateMutexExA(
    attributes: *mut c_void,
    name: *const u8,
    flags: u32,
    access: u32,
) -> Handle {
    let mut buf = [0u16; 260];
    match wide_name(name, &mut buf) {
        Ok(name) => CreateMutexExW(attributes, name, flags, access),
        Err(e) => fail_with(e, core::ptr::null_mut()),
    }
}

/// OpenSemaphoreW(access, inherit, name).
#[unsafe(no_mangle)]
pub extern "system" fn OpenSemaphoreW(access: u32, _inherit: Bool, name: *const u16) -> Handle {
    open_named(name, access, NtOpenSemaphore)
}

#[unsafe(no_mangle)]
pub extern "system" fn OpenSemaphoreA(access: u32, inherit: Bool, name: *const u8) -> Handle {
    if name.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, core::ptr::null_mut());
    }
    let mut buf = [0u16; 260];
    match wide_name(name, &mut buf) {
        Ok(name) => OpenSemaphoreW(access, inherit, name),
        Err(e) => fail_with(e, core::ptr::null_mut()),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn OpenMutexW(access: u32, _inherit: Bool, name: *const u16) -> Handle {
    open_named(name, access, NtOpenMutant)
}

#[unsafe(no_mangle)]
pub extern "system" fn OpenMutexA(access: u32, inherit: Bool, name: *const u8) -> Handle {
    let mut buf = [0u16; 260];
    match wide_name(name, &mut buf) {
        Ok(name) => OpenMutexW(access, inherit, name),
        Err(e) => fail_with(e, core::ptr::null_mut()),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn ReleaseMutex(h: Handle) -> Bool {
    // SAFETY: the kernel validates the handle.
    let status = unsafe { NtReleaseMutant(h, core::ptr::null_mut()) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

/// RTL_CRITICAL_SECTION (40 bytes on x64; on x86 24, smaller than this:
/// only a pointer to it is passed): ntdll's (RtlEnterCriticalSection
/// and friends), so sections work whichever DLL a program calls.
#[repr(C)]
pub struct CriticalSection {
    words: [usize; 5],
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn RtlInitializeCriticalSection(cs: *mut CriticalSection) -> u32;
    fn RtlDeleteCriticalSection(cs: *mut CriticalSection) -> u32;
    fn RtlEnterCriticalSection(cs: *mut CriticalSection) -> u32;
    fn RtlTryEnterCriticalSection(cs: *mut CriticalSection) -> u8;
    fn RtlLeaveCriticalSection(cs: *mut CriticalSection) -> u32;
}

#[unsafe(no_mangle)]
pub extern "system" fn InitializeCriticalSection(cs: *mut CriticalSection) {
    // SAFETY: the caller's CRITICAL_SECTION.
    unsafe { RtlInitializeCriticalSection(cs) };
}

#[unsafe(no_mangle)]
pub extern "system" fn InitializeCriticalSectionAndSpinCount(
    cs: *mut CriticalSection,
    _spin: u32,
) -> Bool {
    InitializeCriticalSection(cs);
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn DeleteCriticalSection(cs: *mut CriticalSection) {
    // SAFETY: the caller's CRITICAL_SECTION.
    unsafe { RtlDeleteCriticalSection(cs) };
}

#[unsafe(no_mangle)]
pub extern "system" fn EnterCriticalSection(cs: *mut CriticalSection) {
    // SAFETY: an initialized CRITICAL_SECTION.
    unsafe { RtlEnterCriticalSection(cs) };
}

#[unsafe(no_mangle)]
pub extern "system" fn TryEnterCriticalSection(cs: *mut CriticalSection) -> Bool {
    // SAFETY: an initialized CRITICAL_SECTION.
    Bool::from(unsafe { RtlTryEnterCriticalSection(cs) } != 0)
}

#[unsafe(no_mangle)]
pub extern "system" fn LeaveCriticalSection(cs: *mut CriticalSection) {
    // SAFETY: called by the owner.
    unsafe { RtlLeaveCriticalSection(cs) };
}

// --- Semaphores ----------------------------------------------------------

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn NtCreateSemaphore(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
        initial: i32,
        maximum: i32,
    ) -> u32;
    fn NtReleaseSemaphore(h: Handle, count: i32, previous: *mut i32) -> u32;
}

const SEMAPHORE_ALL_ACCESS: u32 = 0x1f_0003;

/// CreateSemaphoreW: optionally named (opening an existing semaphore).
#[unsafe(no_mangle)]
pub extern "system" fn CreateSemaphoreW(
    attributes: *mut c_void,
    initial: i32,
    maximum: i32,
    name: *const u16,
) -> Handle {
    if maximum <= 0 || initial < 0 || initial > maximum {
        return fail_with(ERROR_INVALID_PARAMETER, core::ptr::null_mut());
    }
    let mut h = core::ptr::null_mut();
    let status = with_name_sa(name, attributes, |oa| {
        // SAFETY: valid output pointer and attributes.
        unsafe { NtCreateSemaphore(&mut h, SEMAPHORE_ALL_ACCESS, oa, initial, maximum) }
    });
    match status {
        Ok(status) => created(status, h),
        Err(e) => fail_with(e, core::ptr::null_mut()),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateSemaphoreA(
    attributes: *mut c_void,
    initial: i32,
    maximum: i32,
    name: *const u8,
) -> Handle {
    let mut buf = [0u16; 260];
    match wide_name(name, &mut buf) {
        Ok(name) => CreateSemaphoreW(attributes, initial, maximum, name),
        Err(e) => fail_with(e, core::ptr::null_mut()),
    }
}

/// CreateSemaphoreExW: CreateSemaphoreW (flags are reserved; the access
/// mask is not narrowed).
#[unsafe(no_mangle)]
pub extern "system" fn CreateSemaphoreExW(
    attributes: *mut c_void,
    initial: i32,
    maximum: i32,
    name: *const u16,
    _flags: u32,
    _access: u32,
) -> Handle {
    CreateSemaphoreW(attributes, initial, maximum, name)
}

/// ReleaseSemaphore: STATUS_SEMAPHORE_LIMIT_EXCEEDED past the maximum
/// (ERROR_TOO_MANY_POSTS).
#[unsafe(no_mangle)]
pub extern "system" fn ReleaseSemaphore(h: Handle, count: i32, previous: *mut i32) -> Bool {
    // SAFETY: the kernel validates the handle and the output pointer.
    let status = unsafe { NtReleaseSemaphore(h, count, previous) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

/// InitializeCriticalSectionEx: the spin count and flags do not change
/// gala's critical sections.
#[unsafe(no_mangle)]
pub extern "system" fn InitializeCriticalSectionEx(
    cs: *mut CriticalSection,
    _spin: u32,
    _flags: u32,
) -> Bool {
    InitializeCriticalSection(cs);
    TRUE
}

// --- Waiting without a kernel object -------------------------------------
//
// SRW locks, condition variables and InitOnce are one pointer-sized word
// with no kernel object. The SRW locks are ntdll's (the exports forward,
// build.rs), InitOnce calls ntdll's RtlRunOnce*, and a condition variable's
// sleeper waits on its word through ntdll's RtlWaitOnAddress, where every
// wait on a word sleeps (so a word is woken whichever DLL changed it).

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn RtlAcquireSRWLockExclusive(lock: *mut usize);
    fn RtlReleaseSRWLockExclusive(lock: *mut usize);
    fn RtlAcquireSRWLockShared(lock: *mut usize);
    fn RtlReleaseSRWLockShared(lock: *mut usize);
    fn RtlWaitOnAddress(
        address: *const c_void,
        compare: *const c_void,
        size: usize,
        timeout: *const i64,
    ) -> u32;
    fn RtlWakeAddressSingle(address: *const c_void);
    fn RtlWakeAddressAll(address: *const c_void);
    fn RtlRunOnceBeginInitialize(once: *mut usize, flags: u32, context: *mut *mut c_void) -> u32;
    fn RtlRunOnceComplete(once: *mut usize, flags: u32, context: *mut c_void) -> u32;
    fn RtlRunOnceExecuteOnce(
        once: *mut usize,
        f: InitOnceFn,
        param: *mut c_void,
        context: *mut *mut c_void,
    ) -> u32;
}

/// kernel32's own uses of SRW locks (the thread pool): ntdll's.
#[allow(non_snake_case)]
pub(crate) fn AcquireSRWLockExclusive(lock: *mut usize) {
    // SAFETY: the caller's lock word.
    unsafe { RtlAcquireSRWLockExclusive(lock) }
}

#[allow(non_snake_case)]
pub(crate) fn ReleaseSRWLockExclusive(lock: *mut usize) {
    // SAFETY: the caller's lock word, held.
    unsafe { RtlReleaseSRWLockExclusive(lock) }
}

fn srw(lock: *mut usize) -> &'static AtomicUsize {
    // SAFETY: the caller's pointer-sized, aligned word.
    unsafe { AtomicUsize::from_ptr(lock) }
}

/// Spins a few rounds while the word may change soon.
const SPINS: u32 = 64;

/// One step of waiting while `*word == seen`: a spin for the first SPINS
/// steps, then a sleep until the word is woken (or `ms` milliseconds);
/// false if that sleep timed out.
fn wait_step(word: *mut usize, seen: usize, spins: &mut u32, ms: u32) -> bool {
    const STATUS_TIMEOUT: u32 = 0x102;
    if *spins < SPINS {
        *spins += 1;
        core::hint::spin_loop();
        return true;
    }
    let compare = seen;
    let relative = -(i64::from(ms) * 10_000);
    let timeout = if ms == INFINITE { core::ptr::null() } else { &raw const relative };
    // SAFETY: the word and the compare value are pointer-sized; the
    // timeout lives through the call.
    let status = unsafe {
        RtlWaitOnAddress(
            word.cast_const().cast(),
            (&raw const compare).cast(),
            size_of::<usize>(),
            timeout,
        )
    };
    status != STATUS_TIMEOUT
}

fn wake_all(word: *mut usize) {
    // SAFETY: only the address's value is used.
    unsafe { RtlWakeAddressAll(word.cast_const().cast()) };
}

// --- Condition variables -------------------------------------------------
//
// CONDITION_VARIABLE is one pointer, here a wake counter: a sleeper waits
// for it to change. WakeConditionVariable wakes one sleeping waiter; one
// that has not gone to sleep yet sees the counter moved and returns too
// (waits may end spuriously; callers re-check their condition).

const ERROR_TIMEOUT: u32 = 1460;
const CONDITION_VARIABLE_LOCKMODE_SHARED: u32 = 1;

#[unsafe(no_mangle)]
pub extern "system" fn InitializeConditionVariable(cv: *mut usize) {
    srw(cv).store(0, Ordering::Release);
}

#[unsafe(no_mangle)]
pub extern "system" fn WakeConditionVariable(cv: *mut usize) {
    srw(cv).fetch_add(1, Ordering::AcqRel);
    // SAFETY: only the address's value is used.
    unsafe { RtlWakeAddressSingle(cv.cast_const().cast()) };
}

#[unsafe(no_mangle)]
pub extern "system" fn WakeAllConditionVariable(cv: *mut usize) {
    srw(cv).fetch_add(1, Ordering::AcqRel);
    wake_all(cv);
}

/// Waits until `cv` moves past `seen` or `ms` pass; whether it was woken.
fn wait_wake(cv: *mut usize, seen: usize, ms: u32) -> bool {
    let start = GetTickCount64();
    let mut spins = 0;
    loop {
        if srw(cv).load(Ordering::Acquire) != seen {
            return true;
        }
        let left = if ms == INFINITE {
            INFINITE
        } else {
            match u64::from(ms).checked_sub(GetTickCount64() - start) {
                Some(left) if left > 0 => left as u32,
                _ => return false,
            }
        };
        if !wait_step(cv, seen, &mut spins, left) {
            return false;
        }
    }
}

/// SleepConditionVariableCS: releases the critical section (all its
/// recursion), waits, takes it back. FALSE with ERROR_TIMEOUT on timeout.
#[unsafe(no_mangle)]
pub extern "system" fn SleepConditionVariableCS(
    cv: *mut usize,
    cs: *mut CriticalSection,
    ms: u32,
) -> Bool {
    let seen = srw(cv).load(Ordering::Acquire);
    // SAFETY: the caller owns the critical section; RecursionCount is
    // the 32-bit field at offset 12.
    let depth = unsafe { cs.cast::<i32>().add(3).read() };
    for _ in 0..depth {
        LeaveCriticalSection(cs);
    }
    let woken = wait_wake(cv, seen, ms);
    for _ in 0..depth {
        EnterCriticalSection(cs);
    }
    if woken { TRUE } else { fail_with(ERROR_TIMEOUT, FALSE) }
}

/// SleepConditionVariableSRW: as the CS form, with the lock held
/// exclusively or (CONDITION_VARIABLE_LOCKMODE_SHARED) shared.
#[unsafe(no_mangle)]
pub extern "system" fn SleepConditionVariableSRW(
    cv: *mut usize,
    lock: *mut usize,
    ms: u32,
    flags: u32,
) -> Bool {
    let shared = flags & CONDITION_VARIABLE_LOCKMODE_SHARED != 0;
    let seen = srw(cv).load(Ordering::Acquire);
    // SAFETY: the caller's lock, held in the mode the flags say.
    unsafe { if shared { RtlReleaseSRWLockShared(lock) } else { RtlReleaseSRWLockExclusive(lock) } }
    let woken = wait_wake(cv, seen, ms);
    // SAFETY: as above.
    unsafe { if shared { RtlAcquireSRWLockShared(lock) } else { RtlAcquireSRWLockExclusive(lock) } }
    if woken { TRUE } else { fail_with(ERROR_TIMEOUT, FALSE) }
}

// --- One-time initialization ---------------------------------------------
//
// INIT_ONCE is RTL_RUN_ONCE: InitOnceInitialize forwards to ntdll
// (build.rs); the others call ntdll's RtlRunOnce* and give a failing
// status as the last error (STATUS_UNSUCCESSFUL is ERROR_GEN_FAILURE).

type InitOnceFn = extern "system" fn(*mut usize, *mut c_void, *mut *mut c_void) -> Bool;

/// InitOnceExecuteOnce: runs `f` once; other callers wait for it. A
/// callback returning FALSE leaves the once uninitialized and the call
/// returns FALSE without setting the last error (as on Windows).
#[unsafe(no_mangle)]
pub extern "system" fn InitOnceExecuteOnce(
    once: *mut usize,
    f: InitOnceFn,
    param: *mut c_void,
    context: *mut *mut c_void,
) -> Bool {
    // SAFETY: the caller's once, function and context.
    if unsafe { RtlRunOnceExecuteOnce(once, f, param, context) } == STATUS_SUCCESS {
        TRUE
    } else {
        FALSE
    }
}

// --- Interlocked singly linked lists -------------------------------------
//
// SLIST_HEADER is the architecture's (`x64.rs`, `x86.rs`): the first
// entry, the depth and a sequence number, changed together by one
// compare-exchange so a pop cannot be fooled by an entry popped and pushed
// again meanwhile.

use super::arch::{
    SListEntry, SListHeader, slist_cas, slist_depth, slist_first, slist_init, slist_read,
    slist_value,
};

#[unsafe(no_mangle)]
pub extern "system" fn InitializeSListHead(head: *mut SListHeader) {
    slist_init(head);
}

#[unsafe(no_mangle)]
pub extern "system" fn QueryDepthSList(head: *mut SListHeader) -> u16 {
    slist_depth(slist_read(head))
}

#[unsafe(no_mangle)]
pub extern "system" fn InterlockedPushEntrySList(
    head: *mut SListHeader,
    entry: *mut SListEntry,
) -> *mut SListEntry {
    let mut old = slist_read(head);
    loop {
        // SAFETY: the caller's aligned entry, not on any list.
        unsafe { (*entry).next = slist_first(old) };
        let new = slist_value(old, entry, slist_depth(old).wrapping_add(1));
        let seen = slist_cas(head, old, new);
        if seen == old {
            return slist_first(old);
        }
        old = seen;
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn InterlockedPopEntrySList(head: *mut SListHeader) -> *mut SListEntry {
    let mut old = slist_read(head);
    loop {
        let top = slist_first(old);
        if top.is_null() {
            return top;
        }
        // SAFETY: entries stay valid while on the list (the caller's
        // contract); a stale read is caught by the sequence number.
        let next = unsafe { (*top).next };
        let new = slist_value(old, next, slist_depth(old).wrapping_sub(1));
        let seen = slist_cas(head, old, new);
        if seen == old {
            return top;
        }
        old = seen;
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn InterlockedFlushSList(head: *mut SListHeader) -> *mut SListEntry {
    let mut old = slist_read(head);
    loop {
        if slist_first(old).is_null() {
            return core::ptr::null_mut();
        }
        let seen = slist_cas(head, old, slist_value(old, core::ptr::null_mut(), 0));
        if seen == old {
            return slist_first(old);
        }
        old = seen;
    }
}

/// InitOnceBeginInitialize(lpInitOnce, dwFlags, fPending, lpContext):
/// *fPending TRUE for the caller that is to initialize (and must call
/// InitOnceComplete), FALSE with the context once done; others wait (or,
/// INIT_ONCE_ASYNC, initialize too). INIT_ONCE_CHECK_ONLY only reports.
#[unsafe(no_mangle)]
pub extern "system" fn InitOnceBeginInitialize(
    once: *mut usize,
    flags: u32,
    pending: *mut Bool,
    context: *mut *mut c_void,
) -> Bool {
    const STATUS_PENDING: u32 = 0x103;
    // SAFETY: the caller's once and context.
    match unsafe { RtlRunOnceBeginInitialize(once, flags, context) } {
        STATUS_SUCCESS => {
            // SAFETY: the caller's output.
            unsafe { *pending = FALSE };
            TRUE
        }
        STATUS_PENDING => {
            // SAFETY: the caller's output.
            unsafe { *pending = TRUE };
            TRUE
        }
        status => fail(status, FALSE),
    }
}

/// InitOnceComplete(lpInitOnce, dwFlags, lpContext): ends the
/// initialization InitOnceBeginInitialize began: done with the context
/// (its low two bits clear), or with INIT_ONCE_INIT_FAILED back to
/// uninitialized so another caller tries.
#[unsafe(no_mangle)]
pub extern "system" fn InitOnceComplete(
    once: *mut usize,
    flags: u32,
    context: *mut c_void,
) -> Bool {
    // SAFETY: the caller's once.
    match unsafe { RtlRunOnceComplete(once, flags, context) } {
        STATUS_SUCCESS => TRUE,
        status => fail(status, FALSE),
    }
}
