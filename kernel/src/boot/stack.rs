//! The boot thread's stack, measured (`bootstack` on the kernel command
//! line): the bootloader's stack (64 KiB from Limine, ending at
//! `arch::hotpatch::boot_stack_top`) is painted below the boot thread's
//! stack pointer early in `kmain`, and the run's end (`finish`) reports
//! how deep it was ever used: whether a deep path (an early fault's
//! panic, formatted and with its backtrace) comes near its end. Without
//! the flag nothing is painted and only the current depth is reported.

use core::sync::atomic::{AtomicBool, Ordering};

/// The bootloader's stack size (Limine guarantees at least this much).
pub const SIZE: u64 = 64 * 1024;
const PAINT: u64 = 0x5a5a_5a5a_5a5a_5a5a;
/// Left untouched below the stack pointer while painting (this function's
/// own frame and those it calls).
const MARGIN: u64 = 512;

static MARKED: AtomicBool = AtomicBool::new(false);

/// Paints the unused part of the boot stack (once, from the boot thread).
pub fn mark() {
    let top = crate::arch::hotpatch::boot_stack_top();
    if top == 0 {
        return;
    }
    let sp = crate::arch::hotpatch::current_sp();
    let mut at = top - SIZE;
    while at + 8 <= sp - MARGIN {
        // SAFETY: [top - SIZE, sp - MARGIN) is the boot stack's unused
        // part: mapped (the bootloader's stack), below every live frame of
        // this thread, and no other thread runs on it.
        unsafe { core::ptr::with_exposed_provenance_mut::<u64>(at as usize).write_volatile(PAINT) };
        at += 8;
    }
    MARKED.store(true, Ordering::Release);
}

/// The boot stack's use now: (current depth, deepest depth if painted),
/// in bytes below its top; None off the boot stack.
pub fn usage() -> Option<(u64, Option<u64>)> {
    let top = crate::arch::hotpatch::boot_stack_top();
    let sp = crate::arch::hotpatch::current_sp();
    if top == 0 || sp > top || sp < top - SIZE {
        return None;
    }
    let deepest = MARKED.load(Ordering::Acquire).then(|| {
        let mut at = top - SIZE;
        // SAFETY: as `mark`: the boot stack's pages, read up to the
        // current stack pointer.
        while at < sp
            && unsafe { core::ptr::with_exposed_provenance::<u64>(at as usize).read_volatile() }
                == PAINT
        {
            at += 8;
        }
        top - at
    });
    Some((top - sp, deepest))
}

/// `usage` for a log line: ", boot stack 3792 bytes deep (deepest 8 KiB of
/// 64 KiB)", or nothing off the boot stack.
pub struct Usage;

impl core::fmt::Display for Usage {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match usage() {
            None => Ok(()),
            Some((now, None)) => write!(f, "; boot stack {now} bytes deep"),
            Some((now, Some(deepest))) => write!(
                f,
                "; boot stack {now} bytes deep (deepest {} KiB of {} KiB)",
                deepest.div_ceil(1024),
                SIZE / 1024
            ),
        }
    }
}
