//! Exception dispatch, the part x64 and ARM64 share (written from the
//! documented contracts: "Vectored Exception Handling", the Rtl* functions,
//! "SetUnhandledExceptionFilter"): the vectored exception handlers, then
//! the frames' handlers (the architecture's `seh` module walks them), then
//! the vectored continue handlers; an unhandled exception goes to the
//! top-level filter (RtlSetUnhandledExceptionFilter) and, unless it
//! continues execution, ends the process. `raise` is RtlRaiseException's
//! work once the architecture's assembly has captured its caller's
//! CONTEXT.

use crate::seh::CTX_FLAGS;
use crate::{ExceptionPointers, NtContinue, NtTerminateProcess, call_handlers, with_handlers};

/// EXCEPTION_RECORD.ExceptionFlags (the 64-bit layout).
const ER_FLAGS: usize = 0x04;
const EXCEPTION_SOFTWARE_ORIGINATE: u32 = 0x80;
/// ContextFlags: a CONTEXT whose pc is a return address (unwound to a
/// call), not a faulting instruction's (both architectures' bit).
const CONTEXT_UNWOUND_TO_CALL: u32 = 0x2000_0000;

/// RtlRaiseException's work: dispatch the record raised with `context`
/// (its caller's registers); resume it if a handler continued execution.
pub(crate) extern "system" fn raise(record: *mut u8, context: *mut u8) -> ! {
    // Its Rip is the return address of the call that raised: unwound to a
    // call (a hardware exception's context, from the kernel, is not).
    // SAFETY: the caller's CONTEXT (its ContextFlags at CTX_FLAGS).
    unsafe {
        let flags = context.add(CTX_FLAGS).cast::<u32>();
        flags.write_unaligned(flags.read_unaligned() | CONTEXT_UNWOUND_TO_CALL);
        // The record is marked EXCEPTION_SOFTWARE_ORIGINATE (0x80), with
        // whatever flags the caller set: Windows 25H2's 64-bit
        // RtlRaiseException does so itself (a record raised with flags 0
        // reaches the handlers with 0x80; the 32-bit one leaves them).
        let er = record.add(ER_FLAGS).cast::<u32>();
        er.write_unaligned(er.read_unaligned() | EXCEPTION_SOFTWARE_ORIGINATE);
    }
    dispatch_exception(record, context)
}

/// Exception dispatch: the vectored exception handlers, then the frames'
/// handlers; the vectored continue handlers run both when an exception
/// handler continued execution and when none did (assumed from the
/// documented contract and how Go uses them: its last continue handler
/// reports unhandled crashes; to be checked against Windows). Unhandled:
/// the process ends with the exception code.
pub extern "system" fn dispatch_exception(record: *mut u8, context: *mut u8) -> ! {
    let mut pointers = ExceptionPointers { record, context };
    // Snapshots, so that handlers may (un)register handlers themselves.
    let (exception, continue_) = with_handlers(|t| (t.exception, t.continue_));
    let handled =
        call_handlers(&exception, &mut pointers) || crate::seh::dispatch_frames(record, context);
    if call_handlers(&continue_, &mut pointers) || handled {
        NtContinue(context, 0);
    }
    // Unhandled: the top-level filter (kernel32's UnhandledExceptionFilter,
    // which calls SetUnhandledExceptionFilter's) may continue execution;
    // otherwise the process ends.
    let filter = TOP_FILTER.load(core::sync::atomic::Ordering::Acquire);
    if filter != 0 {
        // The kernel keeps it for the crash report: the filter may end
        // the process with a status of its own (Crashpad's 0xffff7002).
        crate::GalaUnhandledException(record, context);
        // SAFETY: a filter registered with RtlSetUnhandledExceptionFilter.
        let f: extern "system" fn(*mut ExceptionPointers) -> i32 =
            unsafe { core::mem::transmute(filter) };
        if f(&mut pointers) == -1 {
            crate::GalaUnhandledException(core::ptr::null(), core::ptr::null());
            NtContinue(context, 0);
        }
    }
    // SAFETY: the record starts with its ExceptionCode; ExceptionAddress
    // is at +0x10 and the parameters at +0x20.
    let (code, address, info) = unsafe {
        (
            (record as *const u32).read_unaligned(),
            record.add(0x10).cast::<u64>().read_unaligned(),
            record.add(0x28).cast::<u64>().read_unaligned(),
        )
    };
    // Gaps are never silent: the ledger has the code and where it was
    // raised (and for access violations, the address accessed).
    let what = b"ntdll.dll!unhandled exception";
    crate::GalaRecordMissing(
        what.as_ptr(),
        what.len(),
        2,
        u64::from(code) << 32 | (address & 0xffff_ffff),
    );
    if code == 0xc000_0005 {
        let what = b"ntdll.dll!unhandled access violation at";
        crate::GalaRecordMissing(what.as_ptr(), what.len(), 2, info);
    }
    NtTerminateProcess(-1, code);
    loop {
        core::hint::spin_loop();
    }
}

/// The top-level unhandled exception filter (0: none).
static TOP_FILTER: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// RtlSetUnhandledExceptionFilter: the filter unhandled exceptions go to
/// (kernel32 sets its UnhandledExceptionFilter).
#[unsafe(no_mangle)]
pub extern "system" fn RtlSetUnhandledExceptionFilter(filter: usize) {
    TOP_FILTER.store(filter, core::sync::atomic::Ordering::Release);
}
