//! Writes through a null pointer. With no exception handler the process
//! must end with STATUS_ACCESS_VIOLATION (0xC0000005), as on Windows.
//!
//! With the argument `filter` (and ntdll, scenario `crash_filtered`) it
//! first registers a top-level filter
//! (RtlSetUnhandledExceptionFilter, what SetUnhandledExceptionFilter
//! reaches) that ends the process itself with 0xFFFF7002, as Chromium's
//! Crashpad does when its handler cannot take the crash: the exit status
//! is the filter's, and the crash report must still describe the access
//! violation.

#![no_std]
#![no_main]

use ntrt::{NtStatus, find_export, find_ntdll, peb};

/// What Crashpad ends a process with when its handler failed.
const FILTER_STATUS: NtStatus = 0xffff_7002;

extern "system" fn filter(_pointers: *mut u8) -> i32 {
    ntrt::print("crash: the top-level filter ends the process\r\n");
    ntrt::exit(FILTER_STATUS)
}

/// Whether the command line ends with the word `filter`.
fn filtered() -> bool {
    // SAFETY: PEB+0x20 holds ProcessParameters; CommandLine is a
    // UNICODE_STRING at +0x70 of it (x64).
    let cmd = unsafe {
        let pp = *(peb().add(0x20) as *const *const u8);
        let len = usize::from(*(pp.add(0x70) as *const u16)) / 2;
        core::slice::from_raw_parts(*(pp.add(0x78) as *const *const u16), len)
    };
    let word = b"filter".map(u16::from);
    let trimmed = cmd.iter().rposition(|&c| c != u16::from(b' ')).map_or(&cmd[..0], |i| &cmd[..=i]);
    trimmed.ends_with(&word)
}

fn main() -> u32 {
    if filtered() {
        let set = find_ntdll().and_then(|n| find_export(n, b"RtlSetUnhandledExceptionFilter"));
        let Some(set) = set else {
            ntrt::print("crash: no RtlSetUnhandledExceptionFilter\r\n");
            return 2;
        };
        // SAFETY: the documented signature.
        let set: extern "system" fn(usize) = unsafe { core::mem::transmute(set) };
        set(filter as *const () as usize);
    }
    ntrt::print("crash: writing to address 0\r\n");
    // SAFETY: deliberately invalid; this faults and never returns.
    unsafe { core::ptr::write_volatile(core::ptr::null_mut::<u32>(), 1) };
    ntrt::print("crash: still alive?!\r\n");
    1
}

ntrt::entry!(main);
