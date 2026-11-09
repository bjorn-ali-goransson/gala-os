//! Calls that catch exceptions, and the call of a server routine with its
//! arguments, in assembly with x64 unwind information (Microsoft's "x64
//! exception handling": the .pdata/.xdata the `.seh_*` directives emit).
//!
//! - `guarded(f, arg)` calls `f(arg)` and returns 0, or, if an exception
//!   that RpcExceptionFilter lets a handler take escapes it, the
//!   exception's code: its frame's handler unwinds to the return path
//!   (RtlUnwindEx with the code as the return value). Exceptions the
//!   filter passes on (access violations, stack overflow, ...) go on to
//!   the process's handlers, as Windows' RPC server lets them.
//! - `call_routine(f, args, count)` calls a server routine with `count`
//!   pointer-sized arguments (the first four in rcx, rdx, r8, r9 and in
//!   xmm0-xmm3 alike, for floating-point parameters; the rest on the
//!   stack above the home area) and returns rax.

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
    ".def rpcrt4_guarded; .scl 2; .type 32; .endef",
    ".globl rpcrt4_guarded",
    ".seh_proc rpcrt4_guarded",
    ".seh_handler rpcrt4_guard_handler, @except",
    "rpcrt4_guarded:",
    "push rbp",
    ".seh_pushreg rbp",
    "sub rsp, 32",
    ".seh_stackalloc 32",
    ".seh_endprologue",
    "mov rax, rcx",
    "mov rcx, rdx",
    "call rax",
    "xor eax, eax",
    ".globl rpcrt4_guard_resume",
    "rpcrt4_guard_resume:",
    "add rsp, 32",
    "pop rbp",
    "ret",
    ".seh_endproc",
    "",
    ".def rpcrt4_call_routine; .scl 2; .type 32; .endef",
    ".globl rpcrt4_call_routine",
    ".seh_proc rpcrt4_call_routine",
    "rpcrt4_call_routine:",
    "push rbp",
    ".seh_pushreg rbp",
    "mov rbp, rsp",
    ".seh_setframe rbp, 0",
    ".seh_endprologue",
    "mov r10, rcx",
    "mov r11, rdx",
    "mov rax, r8",
    "cmp rax, 4",
    "jae .Lrpc_sized",
    "mov rax, 4",
    ".Lrpc_sized:",
    "lea rax, [rax*8 + 15]",
    "and rax, -16",
    "sub rsp, rax",
    "xor ecx, ecx",
    ".Lrpc_copy:",
    "cmp rcx, r8",
    "jae .Lrpc_copied",
    "mov rax, [r11 + rcx*8]",
    "mov [rsp + rcx*8], rax",
    "inc rcx",
    "jmp .Lrpc_copy",
    ".Lrpc_copied:",
    "mov rcx, [r11]",
    "mov rdx, [r11 + 8]",
    "mov r8, [r11 + 16]",
    "mov r9, [r11 + 24]",
    "movq xmm0, rcx",
    "movq xmm1, rdx",
    "movq xmm2, r8",
    "movq xmm3, r9",
    "call r10",
    "lea rsp, [rbp]",
    "pop rbp",
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
    // (HistoryTable at +64).
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

/// Calls server routine `f` with `args` (at least 4 slots readable).
///
/// # Safety
/// `f` takes `args.len()` pointer-sized arguments.
pub unsafe fn call_routine(f: *const c_void, args: &[u64]) -> u64 {
    debug_assert!(args.len() >= 4);
    // SAFETY: as the caller promises.
    unsafe { rpcrt4_call_routine(f, args.as_ptr(), args.len()) }
}
