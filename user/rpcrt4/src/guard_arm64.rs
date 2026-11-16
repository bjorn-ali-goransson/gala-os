//! ARM64's `guard` (x64's is `guard.rs`): calls that catch exceptions, and
//! the call of a server routine with its arguments, in assembly with ARM64
//! unwind information (Microsoft's "ARM64 exception handling": the
//! .pdata/.xdata the `.seh_*` directives emit).
//!
//! - `guarded(f, arg)` calls `f(arg)` and returns 0, or, if an exception
//!   that RpcExceptionFilter lets a handler take escapes it, the
//!   exception's code: its frame's handler unwinds to the return path
//!   (RtlUnwindEx with the code as the return value, which it puts in x0).
//!   Exceptions the filter passes on go on to the process's handlers.
//! - `call_routine(f, args)` calls a server routine with pointer-sized
//!   arguments: the first eight in x0-x7, the rest on the stack, 8 bytes
//!   each. Limit: ARM64 passes floating-point parameters in v0-v7 apart
//!   from the integer ones, so a routine with floating-point parameters
//!   would need their types (the format string's), which this does not
//!   read yet; such a routine gets them in the wrong registers.

use core::ffi::c_void;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn RtlUnwindEx(
        frame: usize,
        target_ip: usize,
        record: *mut u32,
        value: usize,
        context: *mut c_void,
        history: *mut c_void,
    );
}

core::arch::global_asm!(
    ".globl rpcrt4_guarded",
    ".p2align 2",
    "rpcrt4_guarded:",
    ".seh_proc rpcrt4_guarded",
    ".seh_handler rpcrt4_guard_handler, @except",
    "stp x29, x30, [sp, #-16]!",
    ".seh_save_fplr_x 16",
    ".seh_endprologue",
    "mov x16, x0",
    "mov x0, x1",
    "blr x16",
    "mov x0, #0",
    // The handler's unwind resumes here, x0 the exception's code.
    ".globl rpcrt4_guard_resume",
    "rpcrt4_guard_resume:",
    ".seh_startepilogue",
    "ldp x29, x30, [sp], #16",
    ".seh_save_fplr_x 16",
    ".seh_endepilogue",
    "ret",
    ".seh_endproc",
    "",
    // rpcrt4_call_routine(x0 f, x1 args, x2 count; count >= 8 readable).
    ".globl rpcrt4_call_routine",
    ".p2align 2",
    "rpcrt4_call_routine:",
    ".seh_proc rpcrt4_call_routine",
    "stp x29, x30, [sp, #-16]!",
    ".seh_save_fplr_x 16",
    "mov x29, sp",
    ".seh_set_fp",
    ".seh_endprologue",
    "mov x16, x0",
    "mov x17, x1",
    // x9: the stack arguments (count - 8, or none); 16-byte aligned room.
    "subs x9, x2, #8",
    "csel x9, x9, xzr, hi",
    "lsl x10, x9, #3",
    "add x10, x10, #15",
    "and x10, x10, #0xfffffffffffffff0",
    "sub sp, sp, x10",
    "mov x11, #0",
    ".Lrpc_copy:",
    "cmp x11, x9",
    "b.hs .Lrpc_copied",
    "add x12, x11, #8",
    "ldr x13, [x17, x12, lsl #3]",
    "str x13, [sp, x11, lsl #3]",
    "add x11, x11, #1",
    "b .Lrpc_copy",
    ".Lrpc_copied:",
    "ldp x0, x1, [x17]",
    "ldp x2, x3, [x17, #16]",
    "ldp x4, x5, [x17, #32]",
    "ldp x6, x7, [x17, #48]",
    "blr x16",
    ".seh_startepilogue",
    "mov sp, x29",
    ".seh_set_fp",
    "ldp x29, x30, [sp], #16",
    ".seh_save_fplr_x 16",
    ".seh_endepilogue",
    "ret",
    ".seh_endproc",
);

unsafe extern "system" {
    fn rpcrt4_guarded(f: unsafe extern "system" fn(*mut c_void), arg: *mut c_void) -> u32;
    fn rpcrt4_call_routine(f: *const c_void, args: *const u64, count: usize) -> u64;
    static rpcrt4_guard_resume: u8;
}

/// The guard's language handler (EXCEPTION_ROUTINE).
#[unsafe(no_mangle)]
unsafe extern "system" fn rpcrt4_guard_handler(
    record: *mut u32,
    frame: usize,
    context: *mut c_void,
    dispatcher: *mut usize,
) -> u32 {
    const EXCEPTION_UNWIND: u32 = 0x66;
    const CONTINUE_SEARCH: u32 = 1;
    // SAFETY: the dispatcher's EXCEPTION_RECORD and DISPATCHER_CONTEXT
    // (ARM64's: HistoryTable at +64, as x64's).
    unsafe {
        let code = *record;
        let flags = *record.add(1);
        if flags & EXCEPTION_UNWIND != 0 || crate::binding::RpcExceptionFilter(code) == 0 {
            return CONTINUE_SEARCH;
        }
        let history = *dispatcher.add(8) as *mut c_void;
        let target = core::ptr::addr_of!(rpcrt4_guard_resume) as usize;
        RtlUnwindEx(frame, target, record, code as usize, context, history);
    }
    CONTINUE_SEARCH
}

/// `f(arg)`: 0, or the code of the exception that ended it.
pub fn guarded(f: unsafe extern "system" fn(*mut c_void), arg: *mut c_void) -> u32 {
    // SAFETY: `f` is the caller's routine for `arg`.
    unsafe { rpcrt4_guarded(f, arg) }
}

/// Calls server routine `f` with `args`.
///
/// # Safety
/// `f` takes `args.len()` pointer-sized arguments.
pub unsafe fn call_routine(f: *const c_void, args: &[u64]) -> u64 {
    // The assembly loads eight register arguments.
    let mut regs = [0u64; 8];
    let args = if args.len() < 8 {
        regs[..args.len()].copy_from_slice(args);
        &regs[..]
    } else {
        args
    };
    // SAFETY: as the caller promises; at least eight slots readable.
    unsafe { rpcrt4_call_routine(f, args.as_ptr(), args.len()) }
}
