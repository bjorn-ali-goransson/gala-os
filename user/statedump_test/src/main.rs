//! A program that never ends, on purpose: the test of xtask's state dump
//! on a timeout (docs/OBSERVABILITY.md, "State dump"). Its main
//! thread waits for ever on an event nobody sets (NtWaitForSingleObject)
//! and a second thread spins in user code. On Windows it hangs the same
//! way. The boot scenario `statedump` lets it time out after a few
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
