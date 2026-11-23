//! x64's part of apc_test: the frame below the alertable wait whose
//! handler must take the exception an APC raises, and the raise itself.
//!
//! apc_guard(f): pushes rbx, allocates 0x28 bytes, sets rbx, records its
//! establisher frame (its body's rsp on x64) and calls f(); its handler is
//! guard_handler and apc_guard_resume is the unwind target, where rbx says
//! what the unwind restored. apc_raise(record) sets rbx to a value of its
//! own and calls RtlRaiseException, which never returns.

use super::*;
use core::arch::global_asm;

/// rbx in apc_guard's body: what a walk that restored the registers of
/// the frames the APC interrupted leaves there.
pub const GUARD_NV: u64 = 0x3333_0000_0000_3333;
/// rbx while the APC raises, saved in apc_raise's own frame.
const APC_NV: u64 = 0x4444_0000_0000_4444;
/// apc_guard's establisher frame is its body's rsp.
pub const GUARD_FRAME_ABOVE_BODY: u64 = 0;

global_asm!(
    ".text",
    ".globl apc_guard",
    ".globl apc_guard_resume",
    ".def apc_guard; .scl 2; .type 32; .endef",
    "apc_guard:",
    ".seh_proc apc_guard",
    "push rbx",
    ".seh_pushreg rbx",
    "sub rsp, 0x28",
    ".seh_stackalloc 0x28",
    ".seh_endprologue",
    ".seh_handler {handler}, @except, @unwind",
    "mov [rip + {guard_frame}], rsp",
    "mov rbx, {guard_nv}",
    "mov rax, rcx",
    "call rax",
    "apc_guard_resume:",
    "mov [rip + {resume_nv}], rbx",
    "add rsp, 0x28",
    "pop rbx",
    "ret",
    ".seh_endproc",
    "",
    ".globl apc_raise",
    ".def apc_raise; .scl 2; .type 32; .endef",
    "apc_raise:",
    ".seh_proc apc_raise",
    "push rbx",
    ".seh_pushreg rbx",
    "sub rsp, 0x20",
    ".seh_stackalloc 0x20",
    ".seh_endprologue",
    "mov rbx, {apc_nv}",
    "call qword ptr [rip + {raise}]",
    "int3",
    ".seh_endproc",
    handler = sym guard_handler,
    guard_frame = sym GUARD_FRAME,
    resume_nv = sym RESUME_NV,
    raise = sym RTL_RAISE,
    guard_nv = const GUARD_NV,
    apc_nv = const APC_NV,
);

unsafe extern "C" {
    /// Calls `f` in a frame whose handler takes what the APC raises, and
    /// returns the value the unwind to apc_guard_resume carries.
    pub fn apc_guard(f: extern "system" fn() -> u64) -> u64;
    pub fn apc_guard_resume();
    /// RtlRaiseException(record), which does not return here: no handler
    /// of the APC's own takes this exception.
    pub fn apc_raise(record: *mut u8) -> !;
}
