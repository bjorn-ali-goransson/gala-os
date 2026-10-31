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

// --- Semaphores ----------------------------------------------------------

#[link(name = "ntdll", kind = "raw-dylib")]
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
    let status = with_name(name, inheritable(attributes), |oa| {
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
// SRW locks, condition variables and InitOnce have no kernel object: gala
// waits by spinning, then yielding, then sleeping a millisecond at a time.
// TODO: a kernel address wait (as WaitOnAddress) so that long waits sleep.

/// One step of a wait; `spins` counts the steps so far.
fn backoff(spins: &mut u32) {
    *spins = spins.saturating_add(1);
    if *spins < 64 {
        core::hint::spin_loop();
    } else if *spins < 256 {
        SwitchToThread();
    } else {
        Sleep(1);
    }
}

// --- Slim reader/writer locks --------------------------------------------
//
// SRWLOCK is one pointer: 0 when free, EXCLUSIVE when held exclusively,
// otherwise the number of shared holders.

const EXCLUSIVE: usize = 1 << 63;

fn srw(lock: *mut usize) -> &'static AtomicUsize {
    // SAFETY: the caller's pointer-sized, aligned SRWLOCK.
    unsafe { AtomicUsize::from_ptr(lock) }
}

#[unsafe(no_mangle)]
pub extern "system" fn InitializeSRWLock(lock: *mut usize) {
    srw(lock).store(0, Ordering::Release);
}

#[unsafe(no_mangle)]
pub extern "system" fn TryAcquireSRWLockExclusive(lock: *mut usize) -> u8 {
    u8::from(srw(lock).compare_exchange(0, EXCLUSIVE, Ordering::Acquire, Ordering::Relaxed).is_ok())
}

#[unsafe(no_mangle)]
pub extern "system" fn AcquireSRWLockExclusive(lock: *mut usize) {
    let mut spins = 0;
    while TryAcquireSRWLockExclusive(lock) == 0 {
        backoff(&mut spins);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn ReleaseSRWLockExclusive(lock: *mut usize) {
    srw(lock).store(0, Ordering::Release);
}

#[unsafe(no_mangle)]
pub extern "system" fn TryAcquireSRWLockShared(lock: *mut usize) -> u8 {
    let l = srw(lock);
    let mut v = l.load(Ordering::Relaxed);
    loop {
        if v & EXCLUSIVE != 0 {
            return 0;
        }
        match l.compare_exchange_weak(v, v + 1, Ordering::Acquire, Ordering::Relaxed) {
            Ok(_) => return 1,
            Err(now) => v = now,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn AcquireSRWLockShared(lock: *mut usize) {
    let mut spins = 0;
    while TryAcquireSRWLockShared(lock) == 0 {
        backoff(&mut spins);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn ReleaseSRWLockShared(lock: *mut usize) {
    srw(lock).fetch_sub(1, Ordering::Release);
}

// --- Condition variables -------------------------------------------------
//
// CONDITION_VARIABLE is one pointer, here a wake counter: a sleeper waits
// for it to change. Every wake ends every current wait, which the API
// allows (waits may end spuriously; callers re-check their condition).

const ERROR_TIMEOUT: u32 = 1460;
const CONDITION_VARIABLE_LOCKMODE_SHARED: u32 = 1;

#[unsafe(no_mangle)]
pub extern "system" fn InitializeConditionVariable(cv: *mut usize) {
    srw(cv).store(0, Ordering::Release);
}

#[unsafe(no_mangle)]
pub extern "system" fn WakeConditionVariable(cv: *mut usize) {
    srw(cv).fetch_add(1, Ordering::AcqRel);
}

#[unsafe(no_mangle)]
pub extern "system" fn WakeAllConditionVariable(cv: *mut usize) {
    srw(cv).fetch_add(1, Ordering::AcqRel);
}

/// Waits until `cv` moves past `seen` or `ms` pass; whether it was woken.
fn wait_wake(cv: *mut usize, seen: usize, ms: u32) -> bool {
    let start = GetTickCount64();
    let mut spins = 0;
    loop {
        if srw(cv).load(Ordering::Acquire) != seen {
            return true;
        }
        if ms != INFINITE && GetTickCount64() - start >= u64::from(ms) {
            return false;
        }
        backoff(&mut spins);
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
    // SAFETY: the caller owns the critical section.
    let depth = unsafe { (*cs).recursion };
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
    if shared {
        ReleaseSRWLockShared(lock)
    } else {
        ReleaseSRWLockExclusive(lock)
    }
    let woken = wait_wake(cv, seen, ms);
    if shared {
        AcquireSRWLockShared(lock)
    } else {
        AcquireSRWLockExclusive(lock)
    }
    if woken { TRUE } else { fail_with(ERROR_TIMEOUT, FALSE) }
}

// --- One-time initialization ---------------------------------------------
//
// INIT_ONCE is one pointer: 0 before, 1 while a callback runs, and once
// done the context with its low two bits (INIT_ONCE_CTX_RESERVED_BITS)
// replaced by 2.

type InitOnceFn = extern "system" fn(*mut usize, *mut c_void, *mut *mut c_void) -> Bool;

const ONCE_RUNNING: usize = 1;
const ONCE_DONE: usize = 2;

#[unsafe(no_mangle)]
pub extern "system" fn InitOnceInitialize(once: *mut usize) {
    srw(once).store(0, Ordering::Release);
}

/// InitOnceExecuteOnce: runs `f` once; other callers wait for it. A
/// callback returning FALSE leaves the once uninitialized (and the call
/// returns FALSE).
#[unsafe(no_mangle)]
pub extern "system" fn InitOnceExecuteOnce(
    once: *mut usize,
    f: InitOnceFn,
    param: *mut c_void,
    context: *mut *mut c_void,
) -> Bool {
    let state = srw(once);
    let mut spins = 0;
    loop {
        let v = state.load(Ordering::Acquire);
        if v & 3 == ONCE_DONE {
            if !context.is_null() {
                // SAFETY: the caller's output pointer.
                unsafe { *context = (v & !3) as *mut c_void };
            }
            return TRUE;
        }
        if v == 0
            && state.compare_exchange(0, ONCE_RUNNING, Ordering::Acquire, Ordering::Relaxed).is_ok()
        {
            let mut ctx: *mut c_void = core::ptr::null_mut();
            if f(once, param, &mut ctx) == 0 {
                state.store(0, Ordering::Release);
                return FALSE;
            }
            state.store((ctx as usize & !3) | ONCE_DONE, Ordering::Release);
            if !context.is_null() {
                // SAFETY: as above.
                unsafe { *context = (ctx as usize & !3) as *mut c_void };
            }
            return TRUE;
        }
        backoff(&mut spins);
    }
}

// --- Interlocked singly linked lists -------------------------------------
//
// SLIST_HEADER (x64, 16 bytes): the first entry in the high quadword (bits
// 4..63, bit 0 = HeaderType 1), depth (16 bits) and a sequence number (48
// bits) in the low one; both change together with cmpxchg16b, so a pop
// cannot be fooled by an entry popped and pushed again meanwhile.

#[repr(C, align(16))]
pub struct SListHeader {
    low: u64,
    high: u64,
}

#[repr(C)]
pub struct SListEntry {
    next: *mut SListEntry,
}

/// Compares the header with `old` and, if equal, stores `new`; returns
/// the value seen.
fn cas16(head: *mut SListHeader, old: (u64, u64), new: (u64, u64)) -> (u64, u64) {
    let (mut low, mut high) = old;
    // SAFETY: a 16-byte aligned SLIST_HEADER; cmpxchg16b compares rdx:rax
    // with it and stores rcx:rbx if equal, else loads it into rdx:rax.
    // rbx is reserved by LLVM, so it is swapped in and out around the
    // instruction.
    unsafe {
        core::arch::asm!(
            "xchg rbx, {new_low}",
            "lock cmpxchg16b [{head}]",
            "mov rbx, {new_low}",
            head = in(reg) head,
            new_low = inout(reg) new.0 => _,
            in("rcx") new.1,
            inout("rax") low,
            inout("rdx") high,
        );
    }
    (low, high)
}

fn read_header(head: *mut SListHeader) -> (u64, u64) {
    // A cmpxchg16b that fails (or rewrites the same value) reads the
    // header atomically.
    cas16(head, (0, 0), (0, 0))
}

fn first(high: u64) -> *mut SListEntry {
    (high & !0xf) as *mut SListEntry
}

#[unsafe(no_mangle)]
pub extern "system" fn InitializeSListHead(head: *mut SListHeader) {
    // SAFETY: the caller's aligned header.
    unsafe { head.write(SListHeader { low: 0, high: 1 }) };
}

#[unsafe(no_mangle)]
pub extern "system" fn QueryDepthSList(head: *mut SListHeader) -> u16 {
    read_header(head).0 as u16
}

#[unsafe(no_mangle)]
pub extern "system" fn InterlockedPushEntrySList(
    head: *mut SListHeader,
    entry: *mut SListEntry,
) -> *mut SListEntry {
    let mut old = read_header(head);
    loop {
        // SAFETY: the caller's aligned entry, not on any list.
        unsafe { (*entry).next = first(old.1) };
        let depth = (old.0 as u16).wrapping_add(1);
        let sequence = (old.0 >> 16).wrapping_add(1) & 0xffff_ffff_ffff;
        let new = (u64::from(depth) | sequence << 16, entry as u64 | 1);
        let seen = cas16(head, old, new);
        if seen == old {
            return first(old.1);
        }
        old = seen;
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn InterlockedPopEntrySList(head: *mut SListHeader) -> *mut SListEntry {
    let mut old = read_header(head);
    loop {
        let top = first(old.1);
        if top.is_null() {
            return top;
        }
        // SAFETY: entries stay valid while on the list (the caller's
        // contract); a stale read is caught by the sequence number.
        let next = unsafe { (*top).next };
        let depth = (old.0 as u16).wrapping_sub(1);
        let sequence = (old.0 >> 16).wrapping_add(1) & 0xffff_ffff_ffff;
        let new = (u64::from(depth) | sequence << 16, next as u64 | 1);
        let seen = cas16(head, old, new);
        if seen == old {
            return top;
        }
        old = seen;
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn InterlockedFlushSList(head: *mut SListHeader) -> *mut SListEntry {
    let mut old = read_header(head);
    loop {
        if first(old.1).is_null() {
            return core::ptr::null_mut();
        }
        let sequence = (old.0 >> 16).wrapping_add(1) & 0xffff_ffff_ffff;
        let seen = cas16(head, old, (sequence << 16, 1));
        if seen == old {
            return first(old.1);
        }
        old = seen;
    }
}
