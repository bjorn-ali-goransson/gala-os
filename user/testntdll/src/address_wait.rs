//! Waiting on an address: RtlWaitOnAddress, RtlWakeAddressSingle and
//! RtlWakeAddressAll, on one keyed event of this DLL. Everything that
//! sleeps on a word sleeps here: kernelbase's WaitOnAddress and
//! WakeByAddress* (which call these), the SRW locks and one-time
//! initialization (`sync`) and kernel32's condition variables, so a word
//! is woken by whoever changes it, through whichever DLL.
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
//!
//! WAITERS counts the nodes in the list, so that a wake with nobody
//! waiting (most releases of an SRW lock) takes no lock: a waiter counts
//! itself in before it reads the value, a waker changed the value before
//! it reads the count, and both are SeqCst, so a waker that reads 0 knows
//! every waiter to come will see its new value.

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::{
    NtClose, NtCreateKeyedEvent, NtReleaseKeyedEvent, NtWaitForKeyedEvent, NtYieldExecution,
};

const STATUS_SUCCESS: u32 = 0;
const STATUS_INVALID_PARAMETER: u32 = 0xc000_000d;
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
/// Waiters counted in (in the list, or about to check the value).
static WAITERS: AtomicUsize = AtomicUsize::new(0);

fn lock() {
    let mut spins = 0u32;
    while LOCK.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        spins += 1;
        if spins < 64 {
            core::hint::spin_loop();
        } else {
            NtYieldExecution();
        }
    }
}

fn unlock() {
    LOCK.store(false, Ordering::Release);
}

/// The keyed event, or the status of its creation.
fn event() -> Result<usize, u32> {
    let e = EVENT.load(Ordering::Acquire);
    if e != 0 {
        return Ok(e);
    }
    let mut h = 0usize;
    let status = NtCreateKeyedEvent(&mut h, KEYEDEVENT_ALL_ACCESS, core::ptr::null(), 0);
    if status != STATUS_SUCCESS {
        return Err(status);
    }
    match EVENT.compare_exchange(0, h, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => Ok(h),
        Err(other) => {
            // Our own handle, not published.
            NtClose(h);
            Ok(other)
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

/// RtlWaitOnAddress(Address, CompareAddress, AddressSize, Timeout): waits
/// while the value at Address equals the one at CompareAddress, until a
/// wake of Address (or a spurious wake) or the timeout (NULL: none; as
/// NtWaitForKeyedEvent's, negative relative in 100 ns units, else
/// absolute). STATUS_SUCCESS when woken or the value differed,
/// STATUS_TIMEOUT, STATUS_INVALID_PARAMETER for a null pointer or a size
/// not 1, 2, 4 or 8, or the keyed event's creation or wait status.
#[unsafe(no_mangle)]
pub extern "system" fn RtlWaitOnAddress(
    address: *const c_void,
    compare: *const c_void,
    size: usize,
    timeout: *const i64,
) -> u32 {
    if address.is_null() || compare.is_null() || !matches!(size, 1 | 2 | 4 | 8) {
        return STATUS_INVALID_PARAMETER;
    }
    let e = match event() {
        Ok(e) => e,
        Err(status) => return status,
    };
    let mut node = Waiter { address: address as usize, next: core::ptr::null_mut() };
    let w = &raw mut node;
    lock();
    WAITERS.fetch_add(1, Ordering::SeqCst);
    if !same(address.cast(), compare.cast(), size) {
        WAITERS.fetch_sub(1, Ordering::SeqCst);
        unlock();
        return STATUS_SUCCESS;
    }
    // SAFETY: under LOCK; the node outlives its time in the list (it is
    // removed by a waker or below before this function returns).
    unsafe { (*w).next = HEAD.load(Ordering::Relaxed) as *mut Waiter };
    HEAD.store(w as usize, Ordering::Relaxed);
    unlock();
    // The key is the node's (16-aligned) address.
    let status = NtWaitForKeyedEvent(e, w as usize, 0, timeout);
    if status == STATUS_SUCCESS {
        return STATUS_SUCCESS;
    }
    lock();
    let mine = remove(w);
    if mine {
        WAITERS.fetch_sub(1, Ordering::SeqCst);
    }
    unlock();
    if !mine {
        // A waker took the node: its release is coming.
        NtWaitForKeyedEvent(e, w as usize, 0, core::ptr::null());
        return STATUS_SUCCESS;
    }
    status
}

/// RtlWakeAddressSingle(Address): wakes one thread waiting on Address.
#[unsafe(no_mangle)]
pub extern "system" fn RtlWakeAddressSingle(address: *const c_void) {
    wake(address, false);
}

/// RtlWakeAddressAll(Address): wakes every thread waiting on Address.
#[unsafe(no_mangle)]
pub extern "system" fn RtlWakeAddressAll(address: *const c_void) {
    wake(address, true);
}

/// Waits while the word at `word` is `seen` (RtlWaitOnAddress without a
/// timeout); a spurious return is allowed (callers re-check).
pub(crate) fn wait_word(word: *const usize, seen: usize) {
    let compare = seen;
    RtlWaitOnAddress(
        word.cast(),
        (&raw const compare).cast(),
        size_of::<usize>(),
        core::ptr::null(),
    );
}

/// Wakes the waiters on `address` (one, or all).
pub(crate) fn wake(address: *const c_void, all: bool) {
    let address = address as usize;
    // Orders the caller's change of the value before reading the count.
    core::sync::atomic::fence(Ordering::SeqCst);
    if WAITERS.load(Ordering::SeqCst) == 0 {
        return;
    }
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
                WAITERS.fetch_sub(1, Ordering::SeqCst);
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
        // The waiter waits on (or comes back to) this key.
        NtReleaseKeyedEvent(e, taken as usize, 0, core::ptr::null());
        taken = next;
    }
}
