//! Waiting on an address (WaitOnAddress, WakeByAddressSingle and
//! WakeByAddressAll; synchapi.h, API set api-ms-win-core-synch-l1-2-0), on
//! one keyed event per DLL that builds this file.
//!
//! Two DLLs build it, each with its own waiter list and keyed event
//! (ARCHITECTURE "API sets"): kernelbase, which exports the three
//! functions (Windows' kernel32 does not), and kernel32, whose SRW locks,
//! condition variables and InitOnce sleep on their words through it
//! without exporting it. The two never meet: an address is waited on and
//! woken through the same DLL (a kernel32 lock word only by kernel32, a
//! program's address only through kernelbase's exports), so neither needs
//! the other's list. The file takes nothing from the crate that builds it.
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

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn NtCreateKeyedEvent(handle: *mut usize, access: u32, attributes: usize, flags: u32) -> u32;
    fn NtWaitForKeyedEvent(h: usize, key: usize, alertable: u8, timeout: *const i64) -> u32;
    fn NtReleaseKeyedEvent(h: usize, key: usize, alertable: u8, timeout: *const i64) -> u32;
    fn NtClose(h: *mut c_void) -> u32;
    fn NtYieldExecution() -> u32;
    fn RtlNtStatusToDosError(status: u32) -> u32;
}

const STATUS_SUCCESS: u32 = 0;
const STATUS_TIMEOUT: u32 = 0x102;
const INFINITE: u32 = u32::MAX;
const KEYEDEVENT_ALL_ACCESS: u32 = 0xf_0003;

pub const ERROR_INVALID_PARAMETER: u32 = 87;
pub const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
pub const ERROR_TIMEOUT: u32 = 1460;

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
            // SAFETY: no arguments.
            unsafe { NtYieldExecution() };
        }
    }
}

fn unlock() {
    LOCK.store(false, Ordering::Release);
}

/// The keyed event (0 if it cannot be created).
fn event() -> usize {
    let e = EVENT.load(Ordering::Acquire);
    if e != 0 {
        return e;
    }
    let mut h = 0usize;
    // SAFETY: a valid output; no name.
    let status = unsafe { NtCreateKeyedEvent(&mut h, KEYEDEVENT_ALL_ACCESS, 0, 0) };
    if status != STATUS_SUCCESS {
        return 0;
    }
    match EVENT.compare_exchange(0, h, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => h,
        Err(other) => {
            // SAFETY: our own handle, not published.
            unsafe { NtClose(h as *mut c_void) };
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

/// WaitOnAddress's work: waits while the value at `address` equals the one
/// at `compare`, until a wake of `address` (or a spurious wake) or `ms`
/// milliseconds; Ok when woken (or the value differed), else the Win32
/// error WaitOnAddress reports (ERROR_TIMEOUT, ERROR_INVALID_PARAMETER for
/// a null pointer or a size not 1, 2, 4 or 8, ERROR_NOT_ENOUGH_MEMORY
/// without a keyed event, or the wait's status as an error).
pub fn wait(
    address: *const c_void,
    compare: *const c_void,
    size: usize,
    ms: u32,
) -> Result<(), u32> {
    if address.is_null() || compare.is_null() || !matches!(size, 1 | 2 | 4 | 8) {
        return Err(ERROR_INVALID_PARAMETER);
    }
    let e = event();
    if e == 0 {
        return Err(ERROR_NOT_ENOUGH_MEMORY);
    }
    let mut node = Waiter { address: address as usize, next: core::ptr::null_mut() };
    let w = &raw mut node;
    lock();
    WAITERS.fetch_add(1, Ordering::SeqCst);
    if !same(address.cast(), compare.cast(), size) {
        WAITERS.fetch_sub(1, Ordering::SeqCst);
        unlock();
        return Ok(());
    }
    // SAFETY: under LOCK; the node outlives its time in the list (it is
    // removed by a waker or below before this function returns).
    unsafe { (*w).next = HEAD.load(Ordering::Relaxed) as *mut Waiter };
    HEAD.store(w as usize, Ordering::Relaxed);
    unlock();
    let t = (ms != INFINITE).then(|| -(i64::from(ms) * 10_000));
    let t_ptr = t.as_ref().map_or(core::ptr::null(), |t| t as *const i64);
    // SAFETY: the keyed event; the key is the node's (16-aligned) address.
    let status = unsafe { NtWaitForKeyedEvent(e, w as usize, 0, t_ptr) };
    if status == STATUS_SUCCESS {
        return Ok(());
    }
    lock();
    let mine = remove(w);
    if mine {
        WAITERS.fetch_sub(1, Ordering::SeqCst);
    }
    unlock();
    if !mine {
        // A waker took the node: its release is coming.
        // SAFETY: as above, without a timeout.
        unsafe { NtWaitForKeyedEvent(e, w as usize, 0, core::ptr::null()) };
        return Ok(());
    }
    if status == STATUS_TIMEOUT {
        return Err(ERROR_TIMEOUT);
    }
    // SAFETY: a plain conversion.
    Err(unsafe { RtlNtStatusToDosError(status) })
}

/// Wakes the waiters on `address` (one, or all).
pub fn wake(address: *const c_void, all: bool) {
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
        // SAFETY: the keyed event; the waiter waits on (or comes back to) this key.
        unsafe { NtReleaseKeyedEvent(e, taken as usize, 0, core::ptr::null()) };
        taken = next;
    }
}
