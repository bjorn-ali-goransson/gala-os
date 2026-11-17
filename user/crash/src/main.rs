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
//!
//! With the argument `raise` (and ntdll, scenario `crash_raise`) it
//! raises an exception of its own instead: RtlRaiseException with a code
//! no runtime knows and three parameters, which nothing handles. The
//! process must end with that code, as on Windows. Such an exception
//! carries nothing the report can decode but its parameters, so the
//! report must name them -- which is all Office's shared code leaves
//! behind when it raises 0xE0000002 (ROADMAP OF).

#![no_std]
#![no_main]

use ntrt::{NtStatus, find_export, find_ntdll, peb};

/// What Crashpad ends a process with when its handler failed.
const FILTER_STATUS: NtStatus = 0xffff_7002;

/// The code `raise` raises: bit 29 set, so a customer code as Office's
/// 0xE0000002 is, continuable, and none a runtime of its own raises.
const RAISED: u32 = 0xe000_0042;

extern "system" fn filter(_pointers: *mut u8) -> i32 {
    ntrt::print("crash: the top-level filter ends the process\r\n");
    ntrt::exit(FILTER_STATUS)
}

/// Whether the command line's last word is `word`.
fn arg_is<const N: usize>(word: [u8; N]) -> bool {
    // SAFETY: PEB+0x20 holds ProcessParameters; CommandLine is a
    // UNICODE_STRING at +0x70 of it (x64).
    let cmd = unsafe {
        let pp = *(peb().add(0x20) as *const *const u8);
        let len = usize::from(*(pp.add(0x70) as *const u16)) / 2;
        core::slice::from_raw_parts(*(pp.add(0x78) as *const *const u16), len)
    };
    let word = word.map(u16::from);
    let trimmed = cmd.iter().rposition(|&c| c != u16::from(b' ')).map_or(&cmd[..0], |i| &cmd[..=i]);
    trimmed.ends_with(&word)
}

/// Raises RAISED with three parameters and no handler anywhere:
/// RtlRaiseException captures this frame's context, fills the
/// exception's address with the return address here and dispatches.
/// Nothing takes the exception, so the process ends with RAISED.
fn raise() -> u32 {
    let found = find_ntdll().and_then(|n| find_export(n, b"RtlRaiseException"));
    let Some(raise) = found else {
        ntrt::print("crash: no RtlRaiseException\r\n");
        return 2;
    };
    // SAFETY: the documented signature.
    let raise: unsafe extern "system" fn(*mut u8) = unsafe { core::mem::transmute(raise) };
    // EXCEPTION_RECORD (x64 and ARM64): code +0, flags +4, the nested
    // record +8, address +0x10 (RtlRaiseException fills it), the
    // parameter count +0x18, the parameters +0x20. The three: a value
    // that looks like an HRESULT, an address in this image (the report
    // must name the image it is in) and a value of its own.
    let mut record = [0u64; 7];
    record[0] = u64::from(RAISED);
    record[3] = 3;
    record[4] = 0x8007_000e;
    record[5] = main as *const () as u64;
    record[6] = 0xfeed_face;
    ntrt::print("crash: raising 0xe0000042 with three parameters\r\n");
    // SAFETY: a well-formed EXCEPTION_RECORD, and the raise does not
    // return: no handler takes this code.
    unsafe { raise(record.as_mut_ptr().cast()) };
    ntrt::print("crash: the raise returned?!\r\n");
    1
}

fn main() -> u32 {
    if arg_is(*b"raise") {
        return raise();
    }
    if arg_is(*b"filter") {
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
