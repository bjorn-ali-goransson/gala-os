//! ARM64's part of seh_test: the functions under test (register pairs and
//! stack allocations described by ARM64 unwind codes, "ARM64 exception
//! handling"), the CONTEXT's offsets (ARM64_NT_CONTEXT), and the checks of
//! RtlVirtualUnwind from the body, the prologue and the epilogue of
//! seh_outer.
//!
//! seh_outer(f, arg): saves x19 and x20 (`stp x19, x20, [sp, #-32]!`) and
//! the frame record above them, sets x19, records its establisher frame
//! (the sp it was entered with: what unwinding its frame ends with) and
//! calls seh_inner(f, arg); seh_outer_resume is the return point and the
//! unwind target, seh_outer_epilogue its epilogue's first instruction. Its
//! handler is outer_handler. seh_inner(f, arg): saves its frame record and
//! x19, sets x19 and calls f(arg); its handler is inner_handler.
//! seh_raise(record) calls RtlRaiseException; seh_raise_ret is the return
//! address. seh_capture(context) calls RtlCaptureContext with known x19 and
//! x21.

use super::*;
use core::arch::global_asm;

pub const CTX_FLAGS: usize = 0;
/// x19, x20 and x21: the nonvolatile registers the checks follow.
pub const CTX_NV1: usize = 0x08 + 8 * 19;
pub const CTX_NV2: usize = 0x08 + 8 * 20;
pub const CTX_NV3: usize = 0x08 + 8 * 21;
const CTX_FP: usize = 0xf0;
const CTX_LR: usize = 0xf8;
/// How far seh_outer's establisher frame is above its body's stack
/// pointer: ARM64's establisher frame is the sp unwinding seh_outer ends with: 32
/// bytes above its body's (x19, x20 and the frame record).
pub const OUTER_FRAME_ABOVE_BODY: u64 = 32;
pub const CTX_SP: usize = 0x100;
pub const CTX_PC: usize = 0x108;
pub const CONTEXT_SIZE: usize = 0x390;
/// CONTEXT_ARM64 | CONTROL | INTEGER | FLOATING_POINT.
pub const CTX_FULL: u64 = 0x40_0007;
/// RUNTIME_FUNCTION (ARM64): BeginAddress, UnwindData.
pub type Entry = [u32; 2];

global_asm!(
    ".text",
    ".globl seh_outer",
    ".globl seh_outer_resume",
    ".globl seh_outer_epilogue",
    ".p2align 2",
    "seh_outer:",
    ".seh_proc seh_outer",
    "stp x19, x20, [sp, #-32]!",
    ".seh_save_r19r20_x 32",
    "stp x29, x30, [sp, #16]",
    ".seh_save_fplr 16",
    ".seh_endprologue",
    ".seh_handler {outer_handler}, @except, @unwind",
    "add x9, sp, #32",
    "adrp x10, {outer_rsp}",
    "str x9, [x10, :lo12:{outer_rsp}]",
    "movz x19, #{o3}, lsl #48",
    "movk x19, #{o2}, lsl #32",
    "movk x19, #{o1}, lsl #16",
    "movk x19, #{o0}",
    "mov x20, x19",
    "bl seh_inner",
    "seh_outer_resume:",
    "adrp x10, {resume_rbx}",
    "str x19, [x10, :lo12:{resume_rbx}]",
    "adrp x10, {resume_rsi}",
    "str x20, [x10, :lo12:{resume_rsi}]",
    "seh_outer_epilogue:",
    ".seh_startepilogue",
    "ldp x29, x30, [sp, #16]",
    ".seh_save_fplr 16",
    "ldp x19, x20, [sp], #32",
    ".seh_save_r19r20_x 32",
    ".seh_endepilogue",
    "ret",
    ".seh_endproc",
    "",
    ".p2align 2",
    "seh_inner:",
    ".seh_proc seh_inner",
    "stp x29, x30, [sp, #-32]!",
    ".seh_save_fplr_x 32",
    "str x19, [sp, #16]",
    ".seh_save_reg x19, 16",
    "mov x29, sp",
    ".seh_set_fp",
    ".seh_endprologue",
    ".seh_handler {inner_handler}, @except, @unwind",
    "movz x19, #{i3}, lsl #48",
    "movk x19, #{i2}, lsl #32",
    "movk x19, #{i1}, lsl #16",
    "movk x19, #{i0}",
    "mov x9, x0",
    "mov x0, x1",
    "blr x9",
    "nop",
    ".seh_startepilogue",
    "mov sp, x29",
    ".seh_set_fp",
    "ldr x19, [sp, #16]",
    ".seh_save_reg x19, 16",
    "ldp x29, x30, [sp], #32",
    ".seh_save_fplr_x 32",
    ".seh_endepilogue",
    "ret",
    ".seh_endproc",
    "",
    ".globl seh_raise",
    ".globl seh_raise_ret",
    ".p2align 2",
    "seh_raise:",
    ".seh_proc seh_raise",
    "stp x29, x30, [sp, #-16]!",
    ".seh_save_fplr_x 16",
    "mov x29, sp",
    ".seh_set_fp",
    ".seh_endprologue",
    "adrp x9, {raise}",
    "ldr x9, [x9, :lo12:{raise}]",
    "blr x9",
    "seh_raise_ret:",
    "mov x0, #5",
    ".seh_startepilogue",
    "ldp x29, x30, [sp], #16",
    ".seh_save_fplr_x 16",
    ".seh_endepilogue",
    "ret",
    ".seh_endproc",
    "",
    ".globl seh_capture",
    ".globl seh_capture_ret",
    ".p2align 2",
    "seh_capture:",
    ".seh_proc seh_capture",
    "stp x29, x30, [sp, #-48]!",
    ".seh_save_fplr_x 48",
    "stp x19, x20, [sp, #16]",
    ".seh_save_regp x19, 16",
    "str x21, [sp, #32]",
    ".seh_save_reg x21, 32",
    "mov x29, sp",
    ".seh_set_fp",
    ".seh_endprologue",
    "movz x19, #{o3}, lsl #48",
    "movk x19, #{o2}, lsl #32",
    "movk x19, #{o1}, lsl #16",
    "movk x19, #{o0}",
    "movz x21, #{i3}, lsl #48",
    "movk x21, #{i2}, lsl #32",
    "movk x21, #{i1}, lsl #16",
    "movk x21, #{i0}",
    "mov x9, sp",
    "adrp x10, {capture_rsp}",
    "str x9, [x10, :lo12:{capture_rsp}]",
    "adrp x9, {capture}",
    "ldr x9, [x9, :lo12:{capture}]",
    "blr x9",
    "seh_capture_ret:",
    "nop",
    ".seh_startepilogue",
    "ldr x21, [sp, #32]",
    ".seh_save_reg x21, 32",
    "ldp x19, x20, [sp, #16]",
    ".seh_save_regp x19, 16",
    "ldp x29, x30, [sp], #48",
    ".seh_save_fplr_x 48",
    ".seh_endepilogue",
    "ret",
    ".seh_endproc",
    outer_handler = sym outer_handler,
    inner_handler = sym inner_handler,
    outer_rsp = sym OUTER_RSP,
    resume_rbx = sym RESUME_RBX,
    resume_rsi = sym RESUME_RSI,
    capture_rsp = sym CAPTURE_RSP,
    raise = sym RTL_RAISE,
    capture = sym RTL_CAPTURE,
    o0 = const OUTER_RBX & 0xffff,
    o1 = const (OUTER_RBX >> 16) & 0xffff,
    o2 = const (OUTER_RBX >> 32) & 0xffff,
    o3 = const OUTER_RBX >> 48,
    i0 = const INNER_RBX & 0xffff,
    i1 = const (INNER_RBX >> 16) & 0xffff,
    i2 = const (INNER_RBX >> 32) & 0xffff,
    i3 = const INNER_RBX >> 48,
);

unsafe extern "C" {
    pub fn seh_outer(f: extern "system" fn(u64) -> u64, arg: u64) -> u64;
    pub fn seh_outer_resume();
    pub fn seh_outer_epilogue();
    pub fn seh_raise(record: *mut u8) -> u64;
    pub fn seh_raise_ret();
    pub fn seh_capture(context: *mut u8);
    pub fn seh_capture_ret();
}

/// A frame without unwind data (a leaf): its return address is in lr.
pub fn leaf_step(c: *mut u8) {
    wr(c, CTX_PC, rd(c, CTX_LR));
}

/// The link register: the return address until a prologue saves it.
pub fn set_lr(c: *mut u8, lr: u64) {
    wr(c, CTX_LR, lr);
}

/// RtlCaptureContext on ARM64: nothing beyond the shared checks.
pub fn capture_checks(_c: &mut Checks, _cp: *const u8) -> Result<(), u32> {
    Ok(())
}

/// An ARM64 entry with packed unwind data for a function of `len` bytes
/// at `begin` (Flag 1, FunctionLength in instructions, no saves): a lookup
/// reads nothing but the entry.
const fn packed(begin: u32, len: u32) -> Entry {
    [begin, 1 | ((len / 4) << 2)]
}

/// Lookups in function tables registered at run time: a table of two
/// entries (RtlAddFunctionTable), then a growable one, as x64's checks do.
pub fn table_checks(c: &mut Checks, ntdll: usize, fns: &Fns) -> Result<(), u32> {
    let mut base64 = 0u64;
    let (Some(add), Some(delete)) =
        (find_export(ntdll, b"RtlAddFunctionTable"), find_export(ntdll, b"RtlDeleteFunctionTable"))
    else {
        return c.ok(false, 0);
    };
    // SAFETY: the documented signatures.
    let (add, delete) = unsafe {
        (
            core::mem::transmute::<usize, extern "system" fn(*const Entry, u32, u64) -> u8>(add),
            core::mem::transmute::<usize, extern "system" fn(*const Entry) -> u8>(delete),
        )
    };
    // Memory of its own (as a JIT's code is), not inside this image.
    let (s, region, _) = allocate(0, 0x1000, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    c.ok(s == STATUS_SUCCESS && region != 0, s.into())?;
    let region = region as u64;
    let table = [packed(0x10, 0x10), packed(0x40, 0x10)];
    c.ok(add(table.as_ptr(), 2, region) != 0, 0)?;
    let entry = (fns.lookup)(region + 0x18, &mut base64, core::ptr::null_mut());
    c.eq(entry as u64, table.as_ptr() as u64)?;
    c.eq(base64, region)?;
    let entry = (fns.lookup)(region + 0x4c, &mut base64, core::ptr::null_mut());
    c.eq(entry as u64, table.as_ptr() as u64 + 8)?;
    c.ok((fns.lookup)(region + 0x28, &mut base64, core::ptr::null_mut()).is_null(), 0)?;
    c.ok(delete(table.as_ptr()) != 0, 0)?;
    c.ok((fns.lookup)(region + 0x18, &mut base64, core::ptr::null_mut()).is_null(), 0)?;

    // A link record (Flag 3: a jump island at 0x100, its upper 30 bits the
    // RVA of its parent .pdata record): the lookup returns the parent, and
    // unwinding from the island is unwinding from the parent's body. The
    // parent (0x10, 0x20 bytes) allocates 32 bytes (packed: FrameSize 2,
    // CR 0: `sub sp, sp, #32`). The table sits in the region, so the
    // parent's RVA is inside it.
    let island_table = (region + 0x800) as *mut Entry;
    let parent = [0x10, 1 | ((0x20 / 4) << 2) | (2 << 13)];
    // SAFETY: the region's own memory, read-write.
    unsafe {
        island_table.write(parent);
        island_table.add(1).write([0x100, 0x800 | 3]);
    }
    c.ok(add(island_table, 2, region) != 0, 0)?;
    let entry = (fns.lookup)(region + 0x104, &mut base64, core::ptr::null_mut());
    c.eq(entry as u64, island_table as u64)?;
    c.eq(base64, region)?;
    let mut stack = [0u64; 8];
    let mut ctx = Context([0; CONTEXT_SIZE]);
    let (handler, _, frame) =
        super::virtual_unwind(fns, region + 0x104, stack.as_mut_ptr(), 0x7777, &mut ctx)?;
    let cp = ctx.0.as_ptr();
    c.eq(handler, 0)?;
    c.eq(frame, stack.as_ptr() as u64 + 32)?;
    c.eq(rd(cp, CTX_SP), stack.as_ptr() as u64 + 32)?;
    c.eq(rd(cp, CTX_PC), 0x7777)?;
    c.ok(delete(island_table) != 0, 0)?;

    // A growable table: entries relative to RangeBase, only the counted
    // ones searched, more after RtlGrowFunctionTable, none after
    // RtlDeleteGrowableFunctionTable.
    let (Some(add), Some(grow), Some(delete)) = (
        find_export(ntdll, b"RtlAddGrowableFunctionTable"),
        find_export(ntdll, b"RtlGrowFunctionTable"),
        find_export(ntdll, b"RtlDeleteGrowableFunctionTable"),
    ) else {
        return c.ok(false, 0);
    };
    type AddGrowable = extern "system" fn(*mut usize, *mut Entry, u32, u32, usize, usize) -> u32;
    // SAFETY: the documented signatures.
    let (add, grow, delete) = unsafe {
        (
            core::mem::transmute::<usize, AddGrowable>(add),
            core::mem::transmute::<usize, extern "system" fn(usize, u32)>(grow),
            core::mem::transmute::<usize, extern "system" fn(usize)>(delete),
        )
    };
    let mut entries = [packed(0x10, 0x10), [0, 0], [0, 0], [0, 0]];
    let table = entries.as_mut_ptr();
    let region = 0x7_0000_0000u64;
    let mut handle = 0usize;
    c.eq(add(&mut handle, table, 1, 4, region as usize, region as usize + 0x100).into(), 0)?;
    c.ok(handle != 0, 0)?;
    let entry = (fns.lookup)(region + 0x18, &mut base64, core::ptr::null_mut());
    c.eq(entry as u64, table as u64)?;
    c.eq(base64, region)?;
    c.ok((fns.lookup)(region + 0x38, &mut base64, core::ptr::null_mut()).is_null(), 0)?;
    // SAFETY: the table's second slot, written before it is counted.
    unsafe { table.add(1).write_volatile(packed(0x30, 0x10)) };
    grow(handle, 2);
    let entry = (fns.lookup)(region + 0x38, &mut base64, core::ptr::null_mut());
    c.eq(entry as u64, table as u64 + 8)?;
    c.ok((fns.lookup)(region + 0x48, &mut base64, core::ptr::null_mut()).is_null(), 0)?;
    delete(handle);
    c.ok((fns.lookup)(region + 0x18, &mut base64, core::ptr::null_mut()).is_null(), 0)?;
    Ok(())
}

/// RtlVirtualUnwind from seh_outer's body, inside its prologue and from its
/// epilogue. seh_outer's frame (32 bytes below the sp it was entered with):
/// x19 and x20 at sp, the frame record (x29, x30) at sp + 16.
pub fn unwind_checks(c: &mut Checks, fns: &Fns) -> Result<(), u32> {
    let outer_fn = seh_outer as *const () as usize;
    let mut stack = [0u64; 16];
    let sp = stack.as_mut_ptr();
    let put = |i: usize, v: u64| {
        // SAFETY: inside `stack`.
        unsafe { sp.add(i).write_volatile(v) }
    };
    put(0, 0xb0b0); // x19
    put(1, 0x5151); // x20
    put(2, 0xf0f0); // x29
    put(3, 0x1234_5678); // x30: the return address
    let mut ctx = Context([0; CONTEXT_SIZE]);

    // From the body: the codes undo the frame record and the pair; the
    // handler and its data; the establisher frame is the sp it ends with.
    let (handler, data, frame) =
        virtual_unwind(fns, seh_outer_resume as *const () as u64, sp, 0, &mut ctx)?;
    let cp = ctx.0.as_ptr();
    c.eq(handler, outer_handler as *const () as u64)?;
    c.ok(data != 0, data)?;
    c.eq(frame, sp as u64 + 32)?;
    c.eq(rd(cp, CTX_PC), 0x1234_5678)?;
    c.eq(rd(cp, CTX_SP), sp as u64 + 32)?;
    c.eq(rd(cp, CTX_NV1), 0xb0b0)?;
    c.eq(rd(cp, CTX_NV2), 0x5151)?;
    c.eq(rd(cp, CTX_FP), 0xf0f0)?;

    // From inside the prologue, after the first `stp` (one instruction):
    // only that is undone, the return address is still in lr, and no
    // handler is reported.
    let (handler, _, frame) = virtual_unwind(fns, outer_fn as u64 + 4, sp, 0x4321, &mut ctx)?;
    let cp = ctx.0.as_ptr();
    c.eq(handler, 0)?;
    c.eq(frame, sp as u64 + 32)?;
    c.eq(rd(cp, CTX_PC), 0x4321)?;
    c.eq(rd(cp, CTX_SP), sp as u64 + 32)?;
    c.eq(rd(cp, CTX_NV1), 0xb0b0)?;

    // At the start of the epilogue: the epilogue's codes, no handler.
    let (handler, _, _) =
        virtual_unwind(fns, seh_outer_epilogue as *const () as u64, sp, 0, &mut ctx)?;
    let cp = ctx.0.as_ptr();
    c.eq(handler, 0)?;
    c.eq(rd(cp, CTX_PC), 0x1234_5678)?;
    c.eq(rd(cp, CTX_SP), sp as u64 + 32)?;
    c.eq(rd(cp, CTX_NV1), 0xb0b0)?;
    c.eq(rd(cp, CTX_NV2), 0x5151)?;

    // One instruction into the epilogue (after `ldp x29, x30`): the pair
    // remains to be undone.
    put(3, 0x1234_5678);
    let (handler, _, _) =
        virtual_unwind(fns, seh_outer_epilogue as *const () as u64 + 4, sp, 0x8765, &mut ctx)?;
    let cp = ctx.0.as_ptr();
    c.eq(handler, 0)?;
    c.eq(rd(cp, CTX_PC), 0x8765)?;
    c.eq(rd(cp, CTX_SP), sp as u64 + 32)?;
    c.eq(rd(cp, CTX_NV1), 0xb0b0)?;
    Ok(())
}
