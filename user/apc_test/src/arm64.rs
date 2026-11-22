//! ARM64's part of apc_test: the frame below the alertable wait whose
//! handler must take the exception an APC raises, and the raise itself.
//!
//! apc_guard(f): saves x19 and x20 and the frame record above them, sets
//! x19, records its body's sp (its establisher frame is
//! GUARD_FRAME_ABOVE_BODY above that) and calls f(); its handler is
//! guard_handler and apc_guard_resume is the unwind target, where x19 says
//! what the unwind restored. apc_raise(record) sets x19 to a value of its own and calls
//! RtlRaiseException, which never returns.

use super::*;
use core::arch::global_asm;

/// x19 in apc_guard's body: what a walk that restored the registers of
/// the frames the APC interrupted leaves there.
pub const GUARD_NV: u64 = 0x3333_0000_0000_3333;
/// x19 while the APC raises, saved in apc_raise's own frame.
const APC_NV: u64 = 0x4444_0000_0000_4444;
/// ARM64's establisher frame is the sp unwinding the frame ends with: 32
/// bytes above apc_guard's body (x19, x20 and the frame record), which is
/// where its prologue's `stp ..., [sp, #-32]!` came from.
pub const GUARD_FRAME_ABOVE_BODY: u64 = 32;

global_asm!(
    ".text",
    ".globl apc_guard",
    ".globl apc_guard_resume",
    ".p2align 2",
    "apc_guard:",
    ".seh_proc apc_guard",
    "stp x19, x20, [sp, #-32]!",
    ".seh_save_r19r20_x 32",
    "stp x29, x30, [sp, #16]",
    ".seh_save_fplr 16",
    ".seh_endprologue",
    ".seh_handler {handler}, @except, @unwind",
    "mov x9, sp",
    "adrp x10, {guard_frame}",
    "str x9, [x10, :lo12:{guard_frame}]",
    "movz x19, #{g3}, lsl #48",
    "movk x19, #{g2}, lsl #32",
    "movk x19, #{g1}, lsl #16",
    "movk x19, #{g0}",
    "mov x9, x0",
    "blr x9",
    "apc_guard_resume:",
    "adrp x10, {resume_nv}",
    "str x19, [x10, :lo12:{resume_nv}]",
    ".seh_startepilogue",
    "ldp x29, x30, [sp, #16]",
    ".seh_save_fplr 16",
    "ldp x19, x20, [sp], #32",
    ".seh_save_r19r20_x 32",
    ".seh_endepilogue",
    "ret",
    ".seh_endproc",
    "",
    ".globl apc_raise",
    ".p2align 2",
    "apc_raise:",
    ".seh_proc apc_raise",
    "stp x29, x30, [sp, #-32]!",
    ".seh_save_fplr_x 32",
    "str x19, [sp, #16]",
    ".seh_save_reg x19, 16",
    "mov x29, sp",
    ".seh_set_fp",
    ".seh_endprologue",
    "movz x19, #{a3}, lsl #48",
    "movk x19, #{a2}, lsl #32",
    "movk x19, #{a1}, lsl #16",
    "movk x19, #{a0}",
    "adrp x9, {raise}",
    "ldr x9, [x9, :lo12:{raise}]",
    "blr x9",
    "brk #0xf000",
    ".seh_endproc",
    handler = sym guard_handler,
    guard_frame = sym GUARD_FRAME,
    resume_nv = sym RESUME_NV,
    raise = sym RTL_RAISE,
    g0 = const GUARD_NV & 0xffff,
    g1 = const (GUARD_NV >> 16) & 0xffff,
    g2 = const (GUARD_NV >> 32) & 0xffff,
    g3 = const (GUARD_NV >> 48) & 0xffff,
    a0 = const APC_NV & 0xffff,
    a1 = const (APC_NV >> 16) & 0xffff,
    a2 = const (APC_NV >> 32) & 0xffff,
    a3 = const (APC_NV >> 48) & 0xffff,
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
