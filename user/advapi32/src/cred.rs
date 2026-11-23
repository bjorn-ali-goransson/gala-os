//! The credential manager's store, which gala does not have yet.
//!
//! A program reads the user's saved credentials through these: Office's
//! shared runtime asks `CredEnumerateW` for them while it starts, as a
//! *delay load*, so a gala without the export does not hand it a NULL to
//! fall back from -- the delay-load helper raises 0xc06d007f and the
//! process dies (ROADMAP OF). gala keeps no credentials, so what it
//! answers is what a Windows machine with an empty store answers, and
//! every call leaves a row naming what was asked for.
//!
//! Measured on Windows 11 24H2 before any of it was written
//! (`credman_test`): the validation order, the three errors, and that
//! both out parameters are written even by a call that fails.

use core::ffi::c_void;

use super::{fail, missing_detail, reached};

/// Enumerate every credential, filter NULL (learn.microsoft.com,
/// "CredEnumerateW"); mingw-w64's wincred.h does not carry it.
const CRED_ENUMERATE_ALL_CREDENTIALS: u32 = 0x1;

const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_FLAGS: u32 = 1004;
const ERROR_NOT_FOUND: u32 = 1168;

/// What the ledger's detail carries: the flags in the high word and the
/// filter's length in the low one. A length cannot say "no filter" --
/// an empty filter is not a missing one, and Windows answers them
/// differently (`credman_test`) -- so a NULL filter is `u32::MAX`.
fn asked(filter: *const u16, flags: u32) -> u64 {
    const CAP: usize = 4096;
    let len = if filter.is_null() {
        u32::MAX
    } else {
        let mut n = 0usize;
        // SAFETY: the caller's NUL-terminated string. Bounded, so a
        // string without a terminator ends the walk rather than running
        // off into whatever follows it.
        while n < CAP && unsafe { *filter.add(n) } != 0 {
            n += 1;
        }
        n as u32
    };
    u64::from(flags) << 32 | u64::from(len)
}

/// CredEnumerateW(Filter, Flags, Count, Credentials): the credentials
/// whose target name matches `Filter` (a trailing `*` is a prefix
/// match), or every one of them when `Filter` is NULL.
///
/// gala's store is empty, so this is ERROR_NOT_FOUND for every search
/// that gets past the arguments -- the answer a Windows machine with no
/// saved credentials gives, so a program cannot tell the missing store
/// from an empty one, which is the point. The validation is Windows'
/// own, measured (`credman_test`):
///
/// * an unknown flag bit is ERROR_INVALID_PARAMETER, and it beats the
///   known one (3 is refused, not read as 1 with a bit to spare);
/// * CRED_ENUMERATE_ALL_CREDENTIALS wants `Filter` NULL and nothing
///   else: with a filter it is the *flag* that is reported wrong,
///   ERROR_INVALID_FLAGS;
/// * a filter that matches nothing is ERROR_NOT_FOUND, and so is an
///   empty filter: `""` is not NULL;
/// * every failure writes both out parameters anyway -- Count 0 and
///   Credentials NULL -- which a caller can and does read.
///
/// One documented difference: Windows does not validate the out
/// pointers at all, so a NULL `Count` faults inside advapi32. gala
/// refuses it with ERROR_INVALID_PARAMETER and a row instead, because a
/// DLL that answers a bad caller with a fault inside itself buries the
/// caller's bug in gala's module.
#[unsafe(no_mangle)]
pub extern "system" fn CredEnumerateW(
    filter: *const u16,
    flags: u32,
    count: *mut u32,
    credentials: *mut *mut c_void,
) -> i32 {
    let detail = asked(filter, flags);
    if count.is_null() || credentials.is_null() {
        reached(b"advapi32.dll!CredEnumerateW(a null out parameter)", detail);
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    // Written by every answer this function gives, the failures
    // included: the caller's sentinels are gone even on ERROR_NOT_FOUND.
    // SAFETY: the caller's out parameters, both checked above.
    unsafe {
        *count = 0;
        *credentials = core::ptr::null_mut();
    }
    if flags & !CRED_ENUMERATE_ALL_CREDENTIALS != 0 {
        reached(b"advapi32.dll!CredEnumerateW(an unknown flag)", detail);
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    if flags & CRED_ENUMERATE_ALL_CREDENTIALS != 0 && !filter.is_null() {
        reached(b"advapi32.dll!CredEnumerateW(all credentials, with a filter)", detail);
        return fail(ERROR_INVALID_FLAGS, 0);
    }
    // The gap itself: there is no store to search. Reported as missing
    // and not as an answer, because the answer is right while the
    // feature is absent (ROADMAP "The credential store").
    missing_detail(b"advapi32.dll!CredEnumerateW(no credential store)", detail);
    fail(ERROR_NOT_FOUND, 0)
}

/// CredFree(Buffer): frees a buffer a Cred* function handed out -- one
/// call takes an array and everything in it.
///
/// gala's credential functions hand out none (the store is empty, so
/// `CredEnumerateW` only ever writes NULL), so the only pointer a
/// correct caller can bring here is NULL and there is nothing to free.
/// A non-NULL one is not this DLL's to free, so it is reported with its
/// address and left alone: freeing memory gala never allocated would
/// turn a caller's bug into a corrupted heap two allocations later.
#[unsafe(no_mangle)]
pub extern "system" fn CredFree(buffer: *mut c_void) {
    if !buffer.is_null() {
        missing_detail(
            b"advapi32.dll!CredFree(a buffer gala did not hand out)",
            buffer as usize as u64,
        );
    }
}
