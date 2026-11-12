//! Slim reader/writer locks and one-time initialization (RTL_SRWLOCK and
//! RTL_RUN_ONCE: one pointer-sized word each, no kernel object), as
//! Microsoft documents them for ntdll and kernel32 (kernel32's
//! AcquireSRWLock* forward here, its InitOnce* call RtlRunOnce*). A
//! waiter spins briefly, then sleeps on the word (`address_wait`) until
//! it changes; whoever changes it in a way a waiter waits for wakes it
//! (a wake costs no system call when nobody waits).
//!
//! One-time initialization follows the statuses Wine's conformance tests
//! record for Windows (dlls/kernel32/tests/sync.c test_initonce, whose
//! InitOnce* calls are these with the status as the last error).

use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::address_wait::{wait_word, wake};

const STATUS_SUCCESS: u32 = 0;
const STATUS_PENDING: u32 = 0x103;
const STATUS_UNSUCCESSFUL: u32 = 0xc000_0001;
const STATUS_INVALID_PARAMETER: u32 = 0xc000_000d;

/// Spins a few rounds while the word may change soon, then sleeps.
const SPINS: u32 = 64;

fn word(p: *mut usize) -> &'static AtomicUsize {
    // SAFETY: the caller's pointer-sized, aligned lock or once word.
    unsafe { AtomicUsize::from_ptr(p) }
}

/// One step of waiting while `*p == seen`: a spin for the first SPINS
/// steps, then a sleep until the word is woken.
fn wait_step(p: *mut usize, seen: usize, spins: &mut u32) {
    if *spins < SPINS {
        *spins += 1;
        core::hint::spin_loop();
    } else {
        wait_word(p, seen);
    }
}

fn wake_all(p: *mut usize) {
    wake(p.cast_const().cast(), true);
}

// --- Slim reader/writer locks --------------------------------------------
//
// RTL_SRWLOCK is one pointer: 0 when free, EXCLUSIVE when held
// exclusively, otherwise the number of shared holders. No owner is kept
// (any thread may release, as on Windows).

const EXCLUSIVE: usize = 1 << (usize::BITS - 1);

#[unsafe(no_mangle)]
pub extern "system" fn RtlInitializeSRWLock(lock: *mut usize) {
    word(lock).store(0, Ordering::Release);
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlTryAcquireSRWLockExclusive(lock: *mut usize) -> u8 {
    u8::from(
        word(lock).compare_exchange(0, EXCLUSIVE, Ordering::Acquire, Ordering::Relaxed).is_ok(),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlAcquireSRWLockExclusive(lock: *mut usize) {
    let mut spins = 0;
    while RtlTryAcquireSRWLockExclusive(lock) == 0 {
        let held = word(lock).load(Ordering::Relaxed);
        if held != 0 {
            wait_step(lock, held, &mut spins);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlReleaseSRWLockExclusive(lock: *mut usize) {
    word(lock).store(0, Ordering::Release);
    wake_all(lock);
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlTryAcquireSRWLockShared(lock: *mut usize) -> u8 {
    let l = word(lock);
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
pub extern "system" fn RtlAcquireSRWLockShared(lock: *mut usize) {
    let mut spins = 0;
    while RtlTryAcquireSRWLockShared(lock) == 0 {
        let held = word(lock).load(Ordering::Relaxed);
        if held & EXCLUSIVE != 0 {
            wait_step(lock, held, &mut spins);
        }
    }
}

/// The last shared holder wakes the waiters (exclusive ones wait for 0).
#[unsafe(no_mangle)]
pub extern "system" fn RtlReleaseSRWLockShared(lock: *mut usize) {
    if word(lock).fetch_sub(1, Ordering::Release) == 1 {
        wake_all(lock);
    }
}

// --- One-time initialization ---------------------------------------------
//
// RTL_RUN_ONCE is one pointer: 0 before, 1 while a synchronous
// initialization runs, 3 while asynchronous ones run, and once done the
// context with its low two bits (RTL_RUN_ONCE_CTX_RESERVED_BITS) replaced
// by 2.

const RUN_ONCE_CHECK_ONLY: u32 = 1;
const RUN_ONCE_ASYNC: u32 = 2;
const RUN_ONCE_INIT_FAILED: u32 = 4;

const ONCE_RUNNING: usize = 1;
const ONCE_DONE: usize = 2;
const ONCE_ASYNC: usize = 3;

#[unsafe(no_mangle)]
pub extern "system" fn RtlRunOnceInitialize(once: *mut usize) {
    word(once).store(0, Ordering::Release);
}

/// RtlRunOnceBeginInitialize(RunOnce, Flags, Context): STATUS_PENDING for
/// the caller that is to initialize (synchronous: the first one, others
/// wait for it; RTL_RUN_ONCE_ASYNC: every caller until one completes),
/// STATUS_SUCCESS with the context once done. RTL_RUN_ONCE_CHECK_ONLY
/// only reports: STATUS_UNSUCCESSFUL until done. Mixing the synchronous
/// and asynchronous forms, or CHECK_ONLY with ASYNC, is
/// STATUS_INVALID_PARAMETER.
#[unsafe(no_mangle)]
pub extern "system" fn RtlRunOnceBeginInitialize(
    once: *mut usize,
    flags: u32,
    context: *mut *mut c_void,
) -> u32 {
    let state = word(once);
    let done = |v: usize| {
        if !context.is_null() {
            // SAFETY: the caller's output.
            unsafe { *context = (v & !3) as *mut c_void };
        }
        STATUS_SUCCESS
    };
    if flags & RUN_ONCE_CHECK_ONLY != 0 {
        if flags & RUN_ONCE_ASYNC != 0 {
            return STATUS_INVALID_PARAMETER;
        }
        let v = state.load(Ordering::Acquire);
        return if v & 3 == ONCE_DONE { done(v) } else { STATUS_UNSUCCESSFUL };
    }
    let async_ = flags & RUN_ONCE_ASYNC != 0;
    let mut spins = 0;
    loop {
        let v = state.load(Ordering::Acquire);
        match v & 3 {
            0 => {
                let new = if async_ { ONCE_ASYNC } else { ONCE_RUNNING };
                if state.compare_exchange(0, new, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                    return STATUS_PENDING;
                }
            }
            ONCE_RUNNING => {
                if async_ {
                    return STATUS_INVALID_PARAMETER;
                }
                wait_step(once, v, &mut spins);
            }
            ONCE_DONE => return done(v),
            _ => return if async_ { STATUS_PENDING } else { STATUS_INVALID_PARAMETER },
        }
    }
}

/// RtlRunOnceComplete(RunOnce, Flags, Context): ends an initialization
/// RtlRunOnceBeginInitialize began: done with the context (its low two
/// bits clear), or with RTL_RUN_ONCE_INIT_FAILED (no context, not
/// asynchronous) back to uninitialized so another caller tries; the
/// waiters wake. An asynchronous completion needs RTL_RUN_ONCE_ASYNC, and
/// only the first one counts. STATUS_UNSUCCESSFUL when nothing is in
/// progress (or another asynchronous caller completed first).
#[unsafe(no_mangle)]
pub extern "system" fn RtlRunOnceComplete(
    once: *mut usize,
    flags: u32,
    context: *mut c_void,
) -> u32 {
    let context = context as usize;
    if context & 3 != 0 {
        return STATUS_INVALID_PARAMETER;
    }
    let new = if flags & RUN_ONCE_INIT_FAILED != 0 {
        if context != 0 || flags & RUN_ONCE_ASYNC != 0 {
            return STATUS_INVALID_PARAMETER;
        }
        0
    } else {
        context | ONCE_DONE
    };
    let state = word(once);
    let v = state.load(Ordering::Acquire);
    match v {
        ONCE_RUNNING => {
            if state.compare_exchange(v, new, Ordering::AcqRel, Ordering::Relaxed).is_err() {
                // Only the initializing caller completes a synchronous once.
                return STATUS_UNSUCCESSFUL;
            }
            wake_all(once);
            STATUS_SUCCESS
        }
        ONCE_ASYNC => {
            if flags & RUN_ONCE_ASYNC == 0 {
                return STATUS_INVALID_PARAMETER;
            }
            match state.compare_exchange(v, new, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => STATUS_SUCCESS,
                Err(_) => STATUS_UNSUCCESSFUL,
            }
        }
        _ => STATUS_UNSUCCESSFUL,
    }
}

type RunOnceFn = extern "system" fn(*mut usize, *mut c_void, *mut *mut c_void) -> u32;

/// RtlRunOnceExecuteOnce(RunOnce, InitFn, Parameter, Context): runs
/// InitFn once (others wait for it). The function gets the caller's
/// Context as it is (NULL included); what it leaves there is kept with
/// the once. A function returning FALSE leaves the once uninitialized:
/// STATUS_UNSUCCESSFUL.
#[unsafe(no_mangle)]
pub extern "system" fn RtlRunOnceExecuteOnce(
    once: *mut usize,
    f: RunOnceFn,
    param: *mut c_void,
    context: *mut *mut c_void,
) -> u32 {
    let status = RtlRunOnceBeginInitialize(once, 0, context);
    if status != STATUS_PENDING {
        return status;
    }
    if f(once, param, context) == 0 {
        RtlRunOnceComplete(once, RUN_ONCE_INIT_FAILED, core::ptr::null_mut());
        return STATUS_UNSUCCESSFUL;
    }
    let ctx = if context.is_null() {
        core::ptr::null_mut()
    } else {
        // SAFETY: the caller's context, set by the function.
        unsafe { *context }
    };
    RtlRunOnceComplete(once, 0, ctx)
}
