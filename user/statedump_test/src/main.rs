//! A program that never ends, on purpose: the test of xtask's state dump
//! on a timeout (docs/OBSERVABILITY.md, "State dump"). Its main
//! thread waits for ever on an event nobody sets (NtWaitForSingleObject)
//! and a second thread spins in user code; a third waits for ever on a
//! keyed event (as a lock does; the dump names the key, the value there
//! and the waiters queued for it, and the caller of the system call stub
//! with its module). On Windows it hangs the same way. The boot scenario `statedump` lets it time out after a few
//! seconds: xtask then injects an NMI and the kernel's dump must show both
//! threads (the wait with its event, the spinning thread's CPU in user
//! mode), every CPU and the flight recorder; in gala-um, SIGQUIT asks for
//! the same dump.
//!
//! Prints `statedump_test: waiting` once both threads are where they stay.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicBool, Ordering};

use ntrt::*;

const EVENT_ALL_ACCESS: u32 = 0x1f_0003;
const NOTIFICATION_EVENT: u32 = 0;

static SPINNING: AtomicBool = AtomicBool::new(false);

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtCreateKeyedEvent(h: *mut Handle, access: u32, attrs: *const u8, flags: u32) -> NtStatus;
    fn NtWaitForKeyedEvent(h: Handle, key: usize, alertable: u8, timeout: *const i64) -> NtStatus;
}

/// The key the third thread waits on: a lock word's address, as locks use.
static LOCK_WORD: u64 = 0x6a1a;

/// Waits for ever on the keyed event `h` for `LOCK_WORD`'s address.
extern "C" fn keyed_wait(h: usize) -> ! {
    // SAFETY: a keyed event handle; nobody releases the key.
    unsafe { NtWaitForKeyedEvent(Handle(h), &raw const LOCK_WORD as usize, 0, core::ptr::null()) };
    loop {
        core::hint::spin_loop();
    }
}

/// Spins in user code for ever (no system calls).
extern "C" fn spin(_: usize) -> ! {
    SPINNING.store(true, Ordering::Release);
    let mut n: u64 = 0;
    loop {
        n = core::hint::black_box(n.wrapping_add(1));
        core::hint::spin_loop();
    }
}

fn main() -> u32 {
    let (s, event) = create_event(EVENT_ALL_ACCESS, NOTIFICATION_EVENT, false);
    if s != STATUS_SUCCESS {
        return 1;
    }
    let (s, _thread, _) = create_thread(spin, 0, false);
    if s != STATUS_SUCCESS {
        return 2;
    }
    let mut keyed = Handle(0);
    // SAFETY: a fresh keyed event (KEYEDEVENT_ALL_ACCESS).
    if unsafe { NtCreateKeyedEvent(&mut keyed, 0xf_0003, core::ptr::null(), 0) } != STATUS_SUCCESS {
        return 4;
    }
    let (s, _waiter, _) = create_thread(keyed_wait, keyed.0, false);
    if s != STATUS_SUCCESS {
        return 5;
    }
    while !SPINNING.load(Ordering::Acquire) {
        // A millisecond at a time (the spinner may share our processor).
        let _ = wait(event, Some(-10_000));
    }
    print("statedump_test: waiting\r\n");
    // Nobody sets it.
    wait(event, None);
    3
}

entry!(main);
