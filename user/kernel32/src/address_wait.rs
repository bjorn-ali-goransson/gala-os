//! WaitOnAddress, WakeByAddressSingle and WakeByAddressAll (synchapi.h,
//! API set api-ms-win-core-synch-l1-2-0), on one process-wide keyed event.
//!
//! Each waiting thread puts a node (on its own stack) in a list of waiters
//! under a short spin lock, after checking under that lock that the value
//! still equals the compare value; then it waits on the keyed event with
//! its node's address as key. A waker takes matching nodes out of the list
//! under the lock and releases each key. A waiter whose wait timed out
//! takes its node out itself; if a waker already took it, that waker's
//! release is coming (it blocks until the key is waited on), so the
//! waiter waits once more to take it and reports a wake. Spurious wakes
//! are allowed by the documentation (callers re-check the value).

use super::*;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtCreateKeyedEvent(
        handle: *mut usize,
        access: u32,
        attributes: *const ObjectAttributes,
        flags: u32,
    ) -> u32;
    fn NtWaitForKeyedEvent(h: usize, key: usize, alertable: u8, timeout: *const i64) -> u32;
    fn NtReleaseKeyedEvent(h: usize, key: usize, alertable: u8, timeout: *const i64) -> u32;
}

const ERROR_TIMEOUT: u32 = 1460;
const KEYEDEVENT_ALL_ACCESS: u32 = 0xf_0003;

/// A waiting thread's entry (lives on its stack while it waits).
#[repr(C, align(16))]
struct Waiter {
    address: usize,
    next: *mut Waiter,
}

static LOCK: AtomicBool = AtomicBool::new(false);
/// The head of the waiter list (a `*mut Waiter`), under LOCK.
static HEAD: AtomicUsize = AtomicUsize::new(0);
/// The keyed event, created on first use.
static EVENT: AtomicUsize = AtomicUsize::new(0);

fn lock() {
    let mut spins = 0u32;
    while LOCK.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        spins += 1;
        if spins < 64 {
            core::hint::spin_loop();
        } else {
            super::SwitchToThread();
        }
    }
}

fn unlock() {
    LOCK.store(false, Ordering::Release);
}

/// The process's keyed event (0 if it cannot be created).
fn event() -> usize {
    let e = EVENT.load(Ordering::Acquire);
    if e != 0 {
        return e;
    }
    let mut h = 0usize;
    // SAFETY: a valid output; no name.
    let status = unsafe { NtCreateKeyedEvent(&mut h, KEYEDEVENT_ALL_ACCESS, core::ptr::null(), 0) };
    if status != STATUS_SUCCESS {
        return 0;
    }
    match EVENT.compare_exchange(0, h, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => h,
        Err(other) => {
            CloseHandle(h as Handle);
            other
        }
    }
}

/// Removes `w` from the list if it is there (under LOCK); whether it was.
fn remove(w: *mut Waiter) -> bool {
    let mut link = HEAD.as_ptr().cast::<*mut Waiter>();
    // SAFETY: the list's nodes are live waiters' stack entries, changed
    // only under LOCK, which the caller holds.
    unsafe {
        while !(*link).is_null() {
            if *link == w {
                *link = (*w).next;
                return true;
            }
            link = &raw mut (**link).next;
        }
    }
    false
}

/// Whether `size` bytes at `address` equal those at `compare`.
fn same(address: *const u8, compare: *const u8, size: usize) -> bool {
    // SAFETY: the caller's pointers to `size` (1, 2, 4 or 8) bytes.
    unsafe {
        match size {
            1 => core::ptr::read_volatile(address) == *compare,
            2 => {
                core::ptr::read_volatile(address.cast::<u16>())
                    == compare.cast::<u16>().read_unaligned()
            }
            4 => {
                core::ptr::read_volatile(address.cast::<u32>())
                    == compare.cast::<u32>().read_unaligned()
            }
            _ => {
                core::ptr::read_volatile(address.cast::<u64>())
                    == compare.cast::<u64>().read_unaligned()
            }
        }
    }
}

/// WaitOnAddress(Address, CompareAddress, AddressSize, dwMilliseconds):
/// waits while the value at Address equals the one at CompareAddress, until
/// a WakeByAddress* call for Address (or a spurious wake) or the timeout
/// (FALSE, ERROR_TIMEOUT). Sizes 1, 2, 4 and 8.
#[unsafe(no_mangle)]
pub extern "system" fn WaitOnAddress(
    address: *const c_void,
    compare: *const c_void,
    size: usize,
    ms: u32,
) -> Bool {
    if address.is_null() || compare.is_null() || !matches!(size, 1 | 2 | 4 | 8) {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let e = event();
    if e == 0 {
        return fail_with(ERROR_NOT_ENOUGH_MEMORY, FALSE);
    }
    let mut node = Waiter { address: address as usize, next: core::ptr::null_mut() };
    let w = &raw mut node;
    lock();
    if !same(address.cast(), compare.cast(), size) {
        unlock();
        return TRUE;
    }
    // SAFETY: under LOCK; the node outlives its time in the list (it is
    // removed by a waker or below before this function returns).
    unsafe { (*w).next = HEAD.load(Ordering::Relaxed) as *mut Waiter };
    HEAD.store(w as usize, Ordering::Relaxed);
    unlock();
    let t = timeout(ms);
    let t_ptr = t.as_ref().map_or(core::ptr::null(), |t| t as *const i64);
    // SAFETY: the keyed event; the key is the node's (16-aligned) address.
    let status = unsafe { NtWaitForKeyedEvent(e, w as usize, 0, t_ptr) };
    if status == STATUS_SUCCESS {
        return TRUE;
    }
    lock();
    let mine = remove(w);
    unlock();
    if !mine {
        // A waker took the node: its release is coming.
        // SAFETY: as above, without a timeout.
        unsafe { NtWaitForKeyedEvent(e, w as usize, 0, core::ptr::null()) };
        return TRUE;
    }
    if status == STATUS_TIMEOUT {
        return fail_with(ERROR_TIMEOUT, FALSE);
    }
    fail(status, FALSE)
}

/// Wakes the waiters on `address` (one, or all).
fn wake(address: usize, all: bool) {
    let e = EVENT.load(Ordering::Acquire);
    if e == 0 {
        return; // nobody ever waited
    }
    let mut taken: *mut Waiter = core::ptr::null_mut();
    lock();
    let mut link = HEAD.as_ptr().cast::<*mut Waiter>();
    // SAFETY: under LOCK (see `remove`); taken nodes are linked through
    // `next` into `taken` and not touched by their waiters until released.
    unsafe {
        while !(*link).is_null() {
            let w = *link;
            if (*w).address == address {
                *link = (*w).next;
                (*w).next = taken;
                taken = w;
                if !all {
                    break;
                }
            } else {
                link = &raw mut (*w).next;
            }
        }
    }
    unlock();
    while !taken.is_null() {
        // SAFETY: read before the release (the waiter may return after it).
        let next = unsafe { (*taken).next };
        // SAFETY: the keyed event; the waiter waits on (or comes back to) this key.
        unsafe { NtReleaseKeyedEvent(e, taken as usize, 0, core::ptr::null()) };
        taken = next;
    }
}

/// WakeByAddressSingle(Address): wakes one thread waiting on Address.
#[unsafe(no_mangle)]
pub extern "system" fn WakeByAddressSingle(address: *const c_void) {
    wake(address as usize, false);
}

/// WakeByAddressAll(Address): wakes every thread waiting on Address.
#[unsafe(no_mangle)]
pub extern "system" fn WakeByAddressAll(address: *const c_void) {
    wake(address as usize, true);
}
