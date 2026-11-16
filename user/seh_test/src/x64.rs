//! x64's part of seh_test: the functions under test (pushes and stack
//! allocations described by x64 unwind codes), the CONTEXT's offsets, and
//! the checks of x64's own unwind format: the function tables registered
//! at run time (an indirect entry, a growable table) and RtlVirtualUnwind
//! from the body, the prologue and the epilogue of seh_outer.

use super::*;
use core::arch::global_asm;

pub const CTX_FLAGS: usize = 0x30;
const CTX_SEG_CS: usize = 0x38;
/// rbx, rsi and r12: the nonvolatile registers the checks follow.
pub const CTX_NV1: usize = 0x90;
/// How far seh_outer's establisher frame is above its body's stack
/// pointer: x64's establisher frame is the stack pointer of the function's body.
pub const OUTER_FRAME_ABOVE_BODY: u64 = 0;
pub const CTX_SP: usize = 0x98;
pub const CTX_NV2: usize = 0xa8;
pub const CTX_NV3: usize = 0xd8;
pub const CTX_PC: usize = 0xf8;
pub const CONTEXT_SIZE: usize = 0x4d0;
pub const CTX_FULL: u64 = 0x10_000b;
/// RUNTIME_FUNCTION: BeginAddress, EndAddress, UnwindData.
pub type Entry = [u32; 3];

// seh_outer(f, arg): pushes rbx and rsi, allocates 0x28 bytes, sets rbx,
// records its rsp (its establisher frame) and calls seh_inner(f, arg);
// seh_outer_resume is the return point and the unwind target. Its handler
// is outer_handler. seh_inner(f, arg): pushes rbx, allocates 0x20, sets
// rbx and calls f(arg); its handler is inner_handler. seh_raise(record)
// calls RtlRaiseException; seh_raise_ret is the return address.
// seh_capture(context) calls RtlCaptureContext with known rbx and r12.
global_asm!(
    ".text",
    ".globl seh_outer",
    ".globl seh_outer_resume",
    ".globl seh_outer_epilogue",
    ".def seh_outer; .scl 2; .type 32; .endef",
    "seh_outer:",
    ".seh_proc seh_outer",
    "push rbx",
    ".seh_pushreg rbx",
    "push rsi",
    ".seh_pushreg rsi",
    "sub rsp, 0x28",
    ".seh_stackalloc 0x28",
    ".seh_endprologue",
    ".seh_handler {outer_handler}, @except, @unwind",
    "mov [rip + {outer_rsp}], rsp",
    "mov rbx, {outer_rbx}",
    "mov rsi, rbx",
    "call seh_inner",
    "seh_outer_resume:",
    "mov [rip + {resume_rbx}], rbx",
    "mov [rip + {resume_rsi}], rsi",
    "seh_outer_epilogue:",
    "add rsp, 0x28",
    "pop rsi",
    "pop rbx",
    "ret",
    ".seh_endproc",
    "",
    ".def seh_inner; .scl 3; .type 32; .endef",
    "seh_inner:",
    ".seh_proc seh_inner",
    "push rbx",
    ".seh_pushreg rbx",
    "sub rsp, 0x20",
    ".seh_stackalloc 0x20",
    ".seh_endprologue",
    ".seh_handler {inner_handler}, @except, @unwind",
    "mov rbx, {inner_rbx}",
    "mov rax, rcx",
    "mov rcx, rdx",
    "call rax",
    "nop",
    "add rsp, 0x20",
    "pop rbx",
    "ret",
    ".seh_endproc",
    "",
    ".globl seh_raise",
    ".globl seh_raise_ret",
    ".def seh_raise; .scl 2; .type 32; .endef",
    "seh_raise:",
    ".seh_proc seh_raise",
    "sub rsp, 0x28",
    ".seh_stackalloc 0x28",
    ".seh_endprologue",
    "call qword ptr [rip + {raise}]",
    "seh_raise_ret:",
    "mov eax, 5",
    "add rsp, 0x28",
    "ret",
    ".seh_endproc",
    "",
    ".globl seh_capture",
    ".globl seh_capture_ret",
    ".def seh_capture; .scl 2; .type 32; .endef",
    "seh_capture:",
    ".seh_proc seh_capture",
    "push rbx",
    ".seh_pushreg rbx",
    "push r12",
    ".seh_pushreg r12",
    "sub rsp, 0x28",
    ".seh_stackalloc 0x28",
    ".seh_endprologue",
    "mov rbx, {outer_rbx}",
    "mov r12, {inner_rbx}",
    "mov [rip + {capture_rsp}], rsp",
    "call qword ptr [rip + {capture}]",
    "seh_capture_ret:",
    "nop",
    "add rsp, 0x28",
    "pop r12",
    "pop rbx",
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
    outer_rbx = const OUTER_RBX,
    inner_rbx = const INNER_RBX,
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

/// A frame without unwind data (a leaf): its return address is at rsp.
pub fn leaf_step(c: *mut u8) {
    let rsp = rd(c, CTX_SP);
    // SAFETY: a return address on this thread's stack.
    wr(c, CTX_PC, unsafe { *(rsp as *const u64) });
    wr(c, CTX_SP, rsp + 8);
}

/// x64 has no link register: the return address is on the stack.
pub fn set_lr(_c: *mut u8, _lr: u64) {}

/// RtlCaptureContext's segment: user mode's code selector.
pub fn capture_checks(c: &mut Checks, cp: *const u8) -> Result<(), u32> {
    c.eq(rd(cp, CTX_SEG_CS) & 0xffff, 0x33)
}

/// A fake code region with a function table at 0x100: an indirect entry,
/// then the entry it names (its unwind data at 0x180, never read).
static INDIRECT: [u32; 0x50] = {
    let mut r = [0u32; 0x50];
    r[0x40] = 0x10;
    r[0x41] = 0x20;
    r[0x42] = 0x10c | 1;
    r[0x43] = 0x40;
    r[0x44] = 0x50;
    r[0x45] = 0x180;
    r
};

/// Lookups in function tables registered at run time.
pub fn table_checks(c: &mut Checks, ntdll: usize, fns: &Fns) -> Result<(), u32> {
    let mut base64 = 0u64;
    // An indirect entry in a registered table: [0x10, 0x20) names the
    // entry at 0x10c ([0x40, 0x50)); a lookup inside it returns that one.
    let (Some(add), Some(delete)) =
        (find_export(ntdll, b"RtlAddFunctionTable"), find_export(ntdll, b"RtlDeleteFunctionTable"))
    else {
        return c.ok(false, 0);
    };
    // SAFETY: the documented signatures.
    let (add, delete) = unsafe {
        (
            core::mem::transmute::<usize, extern "system" fn(*const [u32; 3], u32, u64) -> u8>(add),
            core::mem::transmute::<usize, extern "system" fn(*const [u32; 3]) -> u8>(delete),
        )
    };
    // The region is memory of its own (as a JIT's code is), not inside
    // this image: a PC in an image is looked up in its .pdata.
    let (s, region, _) = allocate(0, 0x1000, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    c.ok(s == STATUS_SUCCESS && region != 0, s.into())?;
    // SAFETY: the page just committed holds INDIRECT.
    unsafe {
        core::ptr::copy_nonoverlapping(INDIRECT.as_ptr(), region as *mut u32, INDIRECT.len())
    };
    let region = region as u64;
    let table = (region + 0x100) as *const [u32; 3];
    c.ok(add(table, 2, region) != 0, 0)?;
    let entry = (fns.lookup)(region + 0x18, &mut base64, core::ptr::null_mut());
    c.eq(entry as u64, region + 0x10c)?;
    c.eq(base64, region)?;
    c.ok(delete(table) != 0, 0)?;

    // A growable table (RtlAddGrowableFunctionTable: V8 registers its
    // code space's with it): entries relative to RangeBase, covering
    // [RangeBase, RangeEnd); only the first EntryCount are searched until
    // RtlGrowFunctionTable counts more; none after
    // RtlDeleteGrowableFunctionTable.
    let (Some(add), Some(grow), Some(delete)) = (
        find_export(ntdll, b"RtlAddGrowableFunctionTable"),
        find_export(ntdll, b"RtlGrowFunctionTable"),
        find_export(ntdll, b"RtlDeleteGrowableFunctionTable"),
    ) else {
        return c.ok(false, 0);
    };
    type AddGrowable = extern "system" fn(*mut usize, *mut [u32; 3], u32, u32, usize, usize) -> u32;
    // SAFETY: the documented signatures.
    let (add, grow, delete) = unsafe {
        (
            core::mem::transmute::<usize, AddGrowable>(add),
            core::mem::transmute::<usize, extern "system" fn(usize, u32)>(grow),
            core::mem::transmute::<usize, extern "system" fn(usize)>(delete),
        )
    };
    let mut entries = [[0x10u32, 0x20, 0x80], [0, 0, 0], [0, 0, 0], [0, 0, 0]];
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
    unsafe { table.add(1).write_volatile([0x30, 0x40, 0x90]) };
    grow(handle, 2);
    let entry = (fns.lookup)(region + 0x38, &mut base64, core::ptr::null_mut());
    c.eq(entry as u64, table as u64 + 12)?;
    c.ok((fns.lookup)(region + 0x48, &mut base64, core::ptr::null_mut()).is_null(), 0)?;
    delete(handle);
    c.ok((fns.lookup)(region + 0x18, &mut base64, core::ptr::null_mut()).is_null(), 0)?;

    Ok(())
}

/// RtlVirtualUnwind from seh_outer's body, inside its prologue and from its
/// epilogue.
pub fn unwind_checks(c: &mut Checks, fns: &Fns) -> Result<(), u32> {
    let outer_fn = seh_outer as *const () as usize;
    // RtlVirtualUnwind from seh_outer's body: a fake stack with the saved
    // rsi at +0x28, the saved rbx at +0x30 and a return address at +0x38.
    let mut stack = [0u64; 16];
    let sp = stack.as_mut_ptr();
    let put = |i: usize, v: u64| {
        // SAFETY: inside `stack`.
        unsafe { sp.add(i).write_volatile(v) }
    };
    put(5, 0x5151); // rsi
    put(6, 0xb0b0); // rbx
    put(7, 0x1234_5678); // return address
    let mut ctx = Context([0; CONTEXT_SIZE]);
    let (handler, data, frame) =
        virtual_unwind(fns, seh_outer_resume as *const () as u64, sp, 0, &mut ctx)?;
    let cp = ctx.0.as_ptr();
    c.eq(handler, outer_handler as *const () as u64)?;
    c.ok(data != 0, data)?;
    c.eq(frame, sp as u64)?;
    c.eq(rd(cp, CTX_PC), 0x1234_5678)?;
    c.eq(rd(cp, CTX_SP), sp as u64 + 0x40)?;
    c.eq(rd(cp, CTX_NV1), 0xb0b0)?;
    c.eq(rd(cp, CTX_NV2), 0x5151)?;

    // From inside the prologue, after `push rbx` (1 byte): only that push
    // is undone, and no handler is reported.
    put(0, 0xb1b1); // rbx
    put(1, 0x4321); // return address
    let (handler, _, _) = virtual_unwind(fns, outer_fn as u64 + 1, sp, 0, &mut ctx)?;
    let cp = ctx.0.as_ptr();
    c.eq(handler, 0)?;
    c.eq(rd(cp, CTX_PC), 0x4321)?;
    c.eq(rd(cp, CTX_SP), sp as u64 + 0x10)?;
    c.eq(rd(cp, CTX_NV1), 0xb1b1)?;

    // From the epilogue (at `add rsp, 0x28`): emulated, no handler.
    let (handler, _, _) =
        virtual_unwind(fns, seh_outer_epilogue as *const () as u64, sp, 0, &mut ctx)?;
    let cp = ctx.0.as_ptr();
    c.eq(handler, 0)?;
    c.eq(rd(cp, CTX_PC), 0x1234_5678)?;
    c.eq(rd(cp, CTX_SP), sp as u64 + 0x40)?;
    c.eq(rd(cp, CTX_NV1), 0xb0b0)?;
    c.eq(rd(cp, CTX_NV2), 0x5151)?;

    Ok(())
}
