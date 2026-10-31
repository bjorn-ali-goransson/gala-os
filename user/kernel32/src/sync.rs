//! Named events and mutexes (the object names Win32 programs pass live in
//! the session's `\Sessions\1\BaseNamedObjects`, where `Global\` is a link
//! to `\BaseNamedObjects`), and critical sections.

use super::*;
use core::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

#[link(name = "ntdll", kind = "raw-dylib")]
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
    fn NtReleaseMutant(h: Handle, previous: *mut i32) -> u32;
}

const OBJ_OPENIF: u32 = 0x80;
const EVENT_ALL_ACCESS: u32 = 0x1f_0003;
const MUTANT_ALL_ACCESS: u32 = 0x1f_0001;
const SESSION_OBJECTS: &str = "\\Sessions\\1\\BaseNamedObjects\\";

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

/// Whether SECURITY_ATTRIBUTES (nLength, lpSecurityDescriptor,
/// bInheritHandle) ask for an inheritable handle.
pub(crate) fn inheritable(attributes: *mut c_void) -> bool {
    // SAFETY: the caller's SECURITY_ATTRIBUTES (or NULL).
    !attributes.is_null() && unsafe { *attributes.cast::<u8>().add(16).cast::<i32>() } != 0
}

const OBJ_INHERIT: u32 = 0x2;

/// Runs `f` with OBJECT_ATTRIBUTES naming `name` (UTF-16, NUL-terminated)
/// in the session's object directory (none for a null name), with
/// OBJ_INHERIT if `inherit`.
fn with_name<R>(
    name: *const u16,
    inherit: bool,
    f: impl FnOnce(*const ObjectAttributes) -> R,
) -> Result<R, u32> {
    let inherit_bit = if inherit { OBJ_INHERIT } else { 0 };
    if name.is_null() {
        if !inherit {
            return Ok(f(core::ptr::null()));
        }
        let oa = ObjectAttributes {
            length: size_of::<ObjectAttributes>() as u32,
            root_directory: 0,
            object_name: core::ptr::null(),
            attributes: inherit_bit,
            security_descriptor: 0,
            security_quality_of_service: 0,
        };
        return Ok(f(&oa));
    }
    let mut path = [0u16; 300];
    let mut n = 0;
    for c in SESSION_OBJECTS.encode_utf16().chain(wide_slice(name).iter().copied()) {
        *path.get_mut(n).ok_or(ERROR_INVALID_PARAMETER)? = c;
        n += 1;
    }
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
        security_descriptor: 0,
        security_quality_of_service: 0,
    };
    Ok(f(&oa))
}

/// An ANSI name as UTF-16 (None: no name); Err: ERROR_INVALID_PARAMETER.
fn wide_name(name: *const u8, buf: &mut [u16; 260]) -> Result<*const u16, u32> {
    if name.is_null() {
        return Ok(core::ptr::null());
    }
    ansi_to_wide(name, buf).ok_or(ERROR_INVALID_PARAMETER)?;
    Ok(buf.as_ptr())
}

/// The result of creating a possibly named object: the handle, with
/// ERROR_ALREADY_EXISTS when an existing one was opened.
fn created(status: u32, h: Handle) -> Handle {
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
    let status = with_name(name, inheritable(attributes), |oa| {
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
    let status = with_name(name, inheritable(attributes), |oa| {
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

/// RTL_CRITICAL_SECTION (x64, 40 bytes): DebugInfo, LockCount (-1 when
/// free), RecursionCount, OwningThread, LockSemaphore, SpinCount. gala's
/// version spins and yields while the lock is taken (no kernel wait).
#[repr(C)]
pub struct CriticalSection {
    debug_info: usize,
    lock_count: AtomicI32,
    recursion: i32,
    owner: AtomicUsize,
    semaphore: usize,
    spin_count: usize,
}

#[unsafe(no_mangle)]
pub extern "system" fn InitializeCriticalSection(cs: *mut CriticalSection) {
    // SAFETY: the caller's CRITICAL_SECTION.
    unsafe {
        cs.write(CriticalSection {
            debug_info: 0,
            lock_count: AtomicI32::new(-1),
            recursion: 0,
            owner: AtomicUsize::new(0),
            semaphore: 0,
            spin_count: 0,
        })
    };
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
pub extern "system" fn DeleteCriticalSection(_cs: *mut CriticalSection) {}

#[unsafe(no_mangle)]
pub extern "system" fn EnterCriticalSection(cs: *mut CriticalSection) {
    let me = GetCurrentThreadId() as usize;
    // SAFETY: an initialized CRITICAL_SECTION; recursion is only touched
    // by the owner.
    let cs = unsafe { &mut *cs };
    if cs.owner.load(Ordering::Acquire) == me {
        cs.recursion += 1;
        return;
    }
    let mut spins = 0u32;
    while cs.lock_count.compare_exchange(-1, 0, Ordering::Acquire, Ordering::Relaxed).is_err() {
        spins += 1;
        if spins < 100 {
            core::hint::spin_loop();
        } else {
            SwitchToThread();
        }
    }
    cs.owner.store(me, Ordering::Release);
    cs.recursion = 1;
}

#[unsafe(no_mangle)]
pub extern "system" fn TryEnterCriticalSection(cs: *mut CriticalSection) -> Bool {
    let me = GetCurrentThreadId() as usize;
    // SAFETY: as in EnterCriticalSection.
    let cs = unsafe { &mut *cs };
    if cs.owner.load(Ordering::Acquire) == me {
        cs.recursion += 1;
        return TRUE;
    }
    if cs.lock_count.compare_exchange(-1, 0, Ordering::Acquire, Ordering::Relaxed).is_err() {
        return FALSE;
    }
    cs.owner.store(me, Ordering::Release);
    cs.recursion = 1;
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn LeaveCriticalSection(cs: *mut CriticalSection) {
    // SAFETY: called by the owner.
    let cs = unsafe { &mut *cs };
    cs.recursion -= 1;
    if cs.recursion == 0 {
        cs.owner.store(0, Ordering::Release);
        cs.lock_count.store(-1, Ordering::Release);
    }
}
