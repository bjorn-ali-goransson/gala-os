//! Frame-based exception handling on x64, written from Microsoft's
//! documentation ("x64 exception handling": RUNTIME_FUNCTION, UNWIND_INFO
//! and its unwind codes, the epilogue rules, language-specific handlers
//! and DISPATCHER_CONTEXT) and the documented contracts of the Rtl*
//! functions.
//!
//! - `RtlLookupFunctionEntry` finds the RUNTIME_FUNCTION covering a pc
//!   (an image's exception directory, or a table registered with
//!   `RtlAddFunctionTable`); `RtlPcToFileHeader` the image.
//! - `RtlVirtualUnwind` undoes one frame in a CONTEXT: an epilogue is
//!   emulated from its instructions, otherwise the unwind codes are
//!   applied (in a prologue only those whose instruction has run), then
//!   the return address is popped (or a machine frame restored).
//! - Dispatch (`dispatch_frames`, after the vectored handlers) walks the
//!   frames from the exception's context and calls each exception handler
//!   (UNW_FLAG_EHANDLER) until one returns ExceptionContinueExecution.
//! - `RtlUnwindEx` walks from its caller to the target frame calling the
//!   termination handlers (UNW_FLAG_UHANDLER) with EXCEPTION_UNWINDING
//!   (and EXCEPTION_TARGET_UNWIND for the target), then resumes the target
//!   frame at TargetIp with rax = ReturnValue.
//!
//! The functions that start from their caller's registers
//! (`RtlCaptureContext`, `RtlRaiseException`, `RtlUnwindEx`) and the
//! exception dispatcher are assembly with their own unwind information,
//! so that a walk passes through them like any other frame: the
//! dispatcher's describes the CONTEXT the kernel pushed (nonvolatile
//! registers saved in it) and the machine frame above it.
//!
//! Not yet: nested exceptions and collided unwinds (dispositions 2 and 3
//! are treated as ExceptionContinueSearch), function table callbacks,
//! unhandled exception filters (an unhandled exception ends the process
//! with its code), the history table.

use core::arch::global_asm;

use crate::{
    ExceptionPointers, NtContinue, NtTerminateProcess, call_handlers, rd32, with_handlers,
    with_modules,
};

const UNW_FLAG_EHANDLER: u32 = 1;
const UNW_FLAG_UHANDLER: u32 = 2;
const UNW_FLAG_CHAININFO: u32 = 4;

const EXCEPTION_NONCONTINUABLE: u32 = 1;
const EXCEPTION_UNWINDING: u32 = 2;
const EXCEPTION_EXIT_UNWIND: u32 = 4;
const EXCEPTION_TARGET_UNWIND: u32 = 0x20;

const DISPOSITION_CONTINUE_EXECUTION: u32 = 0;

const STATUS_NONCONTINUABLE_EXCEPTION: u32 = 0xc000_0025;
const STATUS_INVALID_UNWIND_TARGET: u32 = 0xc000_0029;
const STATUS_BAD_STACK: u32 = 0xc000_0028;
const STATUS_UNWIND: u32 = 0xc000_0027;

// CONTEXT (public x64 layout).
const CONTEXT_SIZE: usize = 0x4d0;
const CTX_RAX: usize = 0x78;
const CTX_RSP: usize = 0x98;
const CTX_RIP: usize = 0xf8;
/// Integer register `n` in unwind-code numbering (rax, rcx, rdx, rbx,
/// rsp, rbp, rsi, rdi, r8..r15): the CONTEXT keeps them in that order.
const fn ctx_gpr(n: usize) -> usize {
    0x78 + 8 * n
}
/// Xmm register `n` (FltSave.XmmRegisters).
const fn ctx_xmm(n: usize) -> usize {
    0x1a0 + 16 * n
}

// EXCEPTION_RECORD.
const ER_FLAGS: usize = 0x04;
const RECORD_SIZE: usize = 0x98;

/// RUNTIME_FUNCTION: an entry of an exception directory (.pdata).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RuntimeFunction {
    begin: u32,
    end: u32,
    unwind: u32,
}

/// A CONTEXT, 16-byte aligned as the processor's save areas need.
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct Context([u8; CONTEXT_SIZE]);

impl Context {
    fn get(&self, off: usize) -> u64 {
        u64::from_le_bytes(self.0[off..off + 8].try_into().unwrap_or([0; 8]))
    }

    fn set(&mut self, off: usize, v: u64) {
        self.0[off..off + 8].copy_from_slice(&v.to_le_bytes());
    }

    /// A copy of the CONTEXT at `p`.
    fn from_ptr(p: *const u8) -> Self {
        let mut c = Context([0; CONTEXT_SIZE]);
        // SAFETY: callers pass a CONTEXT.
        unsafe { core::ptr::copy_nonoverlapping(p, c.0.as_mut_ptr(), CONTEXT_SIZE) };
        c
    }
}

/// DISPATCHER_CONTEXT (x64).
#[repr(C)]
struct DispatcherContext {
    control_pc: u64,
    image_base: u64,
    function_entry: *const RuntimeFunction,
    establisher_frame: u64,
    target_ip: u64,
    context_record: *mut u8,
    language_handler: usize,
    handler_data: u64,
    history_table: *mut u8,
    scope_index: u32,
    fill0: u32,
}

/// EXCEPTION_ROUTINE: (record, establisher frame, context, dispatcher
/// context) -> EXCEPTION_DISPOSITION.
type LanguageHandler = extern "system" fn(*mut u8, u64, *mut u8, *mut DispatcherContext) -> u32;

fn rd8(addr: usize) -> u8 {
    // SAFETY: callers read inside mapped images.
    unsafe { *(addr as *const u8) }
}

fn rd16(addr: usize) -> u16 {
    // SAFETY: as above.
    unsafe { (addr as *const u16).read_unaligned() }
}

fn stack64(addr: u64) -> u64 {
    // SAFETY: stack slots named by unwind data of the frame being undone.
    unsafe { (addr as *const u64).read_unaligned() }
}

// --- Function tables ---------------------------------------------------

/// Function tables registered with RtlAddFunctionTable: (first entry,
/// entry count, base address).
const MAX_TABLES: usize = 32;

struct Tables(core::cell::UnsafeCell<[(usize, u32, u64); MAX_TABLES]>);

// SAFETY: changed only by RtlAdd/DeleteFunctionTable; callers registering
// code they are about to run do so before running it (no lock yet, like
// the module table).
unsafe impl Sync for Tables {}

static TABLES: Tables = Tables(core::cell::UnsafeCell::new([(0, 0, 0); MAX_TABLES]));

fn with_tables<R>(f: impl FnOnce(&mut [(usize, u32, u64); MAX_TABLES]) -> R) -> R {
    // SAFETY: see `Tables`.
    f(unsafe { &mut *TABLES.0.get() })
}

/// The base of the loaded image containing `pc`, if any.
fn image_of(pc: usize) -> Option<usize> {
    with_modules(|m| {
        m.iter().find_map(|module| {
            let base = module.base;
            if base == 0 || pc < base {
                return None;
            }
            let size = rd32(base + rd32(base + 0x3c) as usize + 24 + 56) as usize; // SizeOfImage
            (pc < base + size).then_some(base)
        })
    })
}

/// Binary search of a sorted RUNTIME_FUNCTION table for `rva`.
fn search(table: *const RuntimeFunction, count: usize, rva: u32) -> *const RuntimeFunction {
    let (mut low, mut high) = (0, count);
    while low < high {
        let mid = (low + high) / 2;
        // SAFETY: inside the table.
        let f = unsafe { &*table.add(mid) };
        if rva < f.begin {
            high = mid;
        } else if rva >= f.end {
            low = mid + 1;
        } else {
            return f;
        }
    }
    core::ptr::null()
}

/// RtlLookupFunctionEntry(ControlPc, *ImageBase, HistoryTable): the
/// RUNTIME_FUNCTION covering `pc` in the loaded module containing it (from
/// its exception directory, sorted by address) or in a registered
/// function table; null for leaf code and addresses outside both.
///
/// # Safety
/// `image_base` must be null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn RtlLookupFunctionEntry(
    pc: u64,
    image_base: *mut u64,
    _history: *mut core::ffi::c_void,
) -> *const RuntimeFunction {
    let (base, entry) = lookup(pc);
    if !entry.is_null() && !image_base.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { *image_base = base };
    }
    entry
}

fn lookup(pc: u64) -> (u64, *const RuntimeFunction) {
    if let Some(base) = image_of(pc as usize) {
        let (rva, size) = crate::directory(base, 3); // IMAGE_DIRECTORY_ENTRY_EXCEPTION
        if rva != 0 {
            let table = (base + rva) as *const RuntimeFunction;
            let f = search(table, size / 12, (pc as usize - base) as u32);
            if !f.is_null() {
                return (base as u64, f);
            }
        }
    }
    let found = with_tables(|t| {
        t.iter().find_map(|&(table, count, base)| {
            if table == 0 || pc < base || pc - base > u64::from(u32::MAX) {
                return None;
            }
            let f = search(table as *const RuntimeFunction, count as usize, (pc - base) as u32);
            (!f.is_null()).then_some((base, f))
        })
    });
    found.unwrap_or((0, core::ptr::null()))
}

/// RtlAddFunctionTable(FunctionTable, EntryCount, BaseAddress): registers
/// unwind data for code outside images (JIT compilers). The entries must
/// be sorted, as for an image.
#[unsafe(no_mangle)]
pub extern "system" fn RtlAddFunctionTable(
    table: *const RuntimeFunction,
    count: u32,
    base: u64,
) -> u8 {
    with_tables(|t| match t.iter_mut().find(|e| e.0 == 0) {
        Some(slot) => {
            *slot = (table as usize, count, base);
            1
        }
        None => 0,
    })
}

/// RtlDeleteFunctionTable(FunctionTable).
#[unsafe(no_mangle)]
pub extern "system" fn RtlDeleteFunctionTable(table: *const RuntimeFunction) -> u8 {
    with_tables(|t| match t.iter_mut().find(|e| e.0 == table as usize && e.0 != 0) {
        Some(slot) => {
            *slot = (0, 0, 0);
            1
        }
        None => 0,
    })
}

/// RtlPcToFileHeader(PcValue, *BaseOfImage): the base of the image
/// containing `pc` (also stored), or null.
///
/// # Safety
/// `base` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn RtlPcToFileHeader(pc: usize, base: *mut usize) -> usize {
    let found = image_of(pc).unwrap_or(0);
    if !base.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { *base = found };
    }
    found
}

// --- Virtual unwinding -------------------------------------------------

/// KNONVOLATILE_CONTEXT_POINTERS: where each register was restored from.
struct Pointers(*mut u64);

impl Pointers {
    fn integer(&self, reg: usize, addr: u64) {
        if !self.0.is_null() {
            // SAFETY: the caller's structure: FloatingContext[16] then
            // IntegerContext[16].
            unsafe { *self.0.add(16 + reg) = addr };
        }
    }

    fn xmm(&self, reg: usize, addr: u64) {
        if !self.0.is_null() {
            // SAFETY: as above.
            unsafe { *self.0.add(reg) = addr };
        }
    }
}

fn restore_gpr(ctx: &mut Context, reg: usize, addr: u64, ptrs: &Pointers) {
    ctx.set(ctx_gpr(reg), stack64(addr));
    ptrs.integer(reg, addr);
}

fn restore_xmm(ctx: &mut Context, reg: usize, addr: u64, ptrs: &Pointers) {
    let lo = stack64(addr);
    let hi = stack64(addr + 8);
    ctx.set(ctx_xmm(reg), lo);
    ctx.set(ctx_xmm(reg) + 8, hi);
    ptrs.xmm(reg, addr);
}

/// If `pc` is in an epilogue of the function (`add rsp, n` or `lea rsp,
/// [frame register + n]`, then pops of nonvolatile registers, then `ret`
/// or a jump out of the function), emulates the rest of it and returns
/// true. These are the only instruction forms an x64 epilogue may use.
fn unwind_epilogue(
    ctx: &mut Context,
    pc: usize,
    function: (usize, usize),
    frame_reg: usize,
    ptrs: &Pointers,
) -> bool {
    let mut p = pc;
    let mut rsp = ctx.get(CTX_RSP);
    // The stack adjustment, if the pc is on it.
    if rd8(p) == 0x48 && rd8(p + 1) == 0x83 && rd8(p + 2) == 0xc4 {
        rsp = rsp.wrapping_add(u64::from(rd8(p + 3)));
        p += 4;
    } else if rd8(p) == 0x48 && rd8(p + 1) == 0x81 && rd8(p + 2) == 0xc4 {
        rsp = rsp.wrapping_add(u64::from(rd32(p + 3)));
        p += 7;
    } else if (rd8(p) & 0xfe) == 0x48
        && rd8(p + 1) == 0x8d
        && (rd8(p + 2) & 0x38) == 0x20
        && (rd8(p + 2) & 7) != 4
    {
        // lea rsp, [reg + disp8/disp32]
        let reg = usize::from(rd8(p + 2) & 7) + if rd8(p) & 1 != 0 { 8 } else { 0 };
        let (disp, len) = match rd8(p + 2) >> 6 {
            1 => (i64::from(rd8(p + 3) as i8), 4),
            2 => (i64::from(rd32(p + 3) as i32), 7),
            _ => return false,
        };
        if frame_reg == 0 || reg != frame_reg {
            return false;
        }
        rsp = ctx.get(ctx_gpr(reg)).wrapping_add(disp as u64);
        p += len;
    }
    // Pops, then the return or the jump.
    let mut pops = [0usize; 16];
    let mut n = 0;
    loop {
        let (op, len, rex_b) = if rd8(p) == 0x41 { (rd8(p + 1), 2, 8) } else { (rd8(p), 1, 0) };
        if (0x58..=0x5f).contains(&op) && n < pops.len() {
            pops[n] = usize::from(op - 0x58) + rex_b;
            n += 1;
            p += len;
            continue;
        }
        break;
    }
    let ends = match (rd8(p), rd8(p + 1)) {
        (0xc3, _) | (0xc2, _) | (0xf3, 0xc3) => true,
        (0xe9, _) => {
            // A tail call: a jump to outside the function.
            let target = (p + 5).wrapping_add(rd32(p + 1) as i32 as isize as usize);
            target < function.0 || target >= function.1
        }
        (0xff, 0x25) | (0x48, 0xff) => true,
        _ => false,
    };
    if !ends {
        return false;
    }
    for &reg in &pops[..n] {
        restore_gpr(ctx, reg, rsp, ptrs);
        rsp += 8;
    }
    ctx.set(CTX_RIP, stack64(rsp));
    ctx.set(CTX_RSP, rsp + 8);
    true
}

/// Slots (16-bit) an unwind code takes.
fn code_slots(op: u8, info: u8) -> usize {
    match op {
        1 => 2 + usize::from(info != 0), // ALLOC_LARGE
        4 | 8 => 2,                      // SAVE_NONVOL, SAVE_XMM128
        5 | 9 => 3,                      // the _FAR forms
        _ => 1,
    }
}

/// The unwinding behind RtlVirtualUnwind; returns the handler (or 0) and
/// its data and the establisher frame.
fn virtual_unwind(
    kind: u32,
    base: usize,
    pc: usize,
    entry: *const RuntimeFunction,
    ctx: &mut Context,
    ptrs: &Pointers,
) -> (usize, u64, u64) {
    // SAFETY: the caller's RUNTIME_FUNCTION.
    let mut f = unsafe { *entry };
    // An unwind field with bit 0 set names another RUNTIME_FUNCTION (the
    // entry shares its unwind data).
    while f.unwind & 1 != 0 {
        // SAFETY: inside the image's .pdata.
        f = unsafe { *((base + (f.unwind & !1) as usize) as *const RuntimeFunction) };
    }
    let function = (base + f.begin as usize, base + f.end as usize);
    let pc_offset = pc.wrapping_sub(function.0);
    let mut info = base + f.unwind as usize;
    let first_frame_reg = usize::from(rd8(info + 3) & 0xf);
    let prolog = usize::from(rd8(info + 1));
    let in_prologue = pc_offset < prolog;

    // The establisher frame: the frame pointer (less its offset) once the
    // prologue has set it, otherwise rsp.
    let establisher = |ctx: &Context, info: usize, primary: bool| -> u64 {
        let reg = usize::from(rd8(info + 3) & 0xf);
        let offset = u64::from(rd8(info + 3) >> 4) * 16;
        if reg == 0 {
            return ctx.get(CTX_RSP);
        }
        if primary && in_prologue {
            // Has the SET_FPREG instruction run?
            let count = usize::from(rd8(info + 2));
            let set = (0..count).any(|i| {
                let c = info + 4 + 2 * i;
                rd8(c + 1) & 0xf == 3 && usize::from(rd8(c)) <= pc_offset
            });
            if !set {
                return ctx.get(CTX_RSP);
            }
        }
        ctx.get(ctx_gpr(reg)).wrapping_sub(offset)
    };
    let frame = establisher(ctx, info, true);

    if !in_prologue && unwind_epilogue(ctx, pc, function, first_frame_reg, ptrs) {
        return (0, 0, frame);
    }

    let mut primary = true;
    let mut machine_frame = false;
    loop {
        let flags = u32::from(rd8(info) >> 3);
        let count = usize::from(rd8(info + 2));
        let frame_reg = usize::from(rd8(info + 3) & 0xf);
        let frame_offset = u64::from(rd8(info + 3) >> 4) * 16;
        let base_frame = establisher(ctx, info, primary);
        let mut i = 0;
        while i < count {
            let c = info + 4 + 2 * i;
            let (offset, op, op_info) = (usize::from(rd8(c)), rd8(c + 1) & 0xf, rd8(c + 1) >> 4);
            let slots = code_slots(op, op_info);
            if primary && in_prologue && offset > pc_offset {
                i += slots;
                continue;
            }
            let slot16 = |k: usize| u64::from(rd16(c + 2 * k));
            let slot32 = |k: usize| u64::from(rd32(c + 2 * k));
            let rsp = ctx.get(CTX_RSP);
            let reg = usize::from(op_info);
            match op {
                0 => {
                    restore_gpr(ctx, reg, rsp, ptrs);
                    ctx.set(CTX_RSP, rsp + 8);
                }
                1 => {
                    let size = if op_info == 0 { slot16(1) * 8 } else { slot32(1) };
                    ctx.set(CTX_RSP, rsp + size);
                }
                2 => ctx.set(CTX_RSP, rsp + u64::from(op_info) * 8 + 8),
                3 => ctx.set(CTX_RSP, ctx.get(ctx_gpr(frame_reg)).wrapping_sub(frame_offset)),
                4 => restore_gpr(ctx, reg, base_frame + slot16(1) * 8, ptrs),
                5 => restore_gpr(ctx, reg, base_frame + slot32(1), ptrs),
                8 => restore_xmm(ctx, reg, base_frame + slot16(1) * 16, ptrs),
                9 => restore_xmm(ctx, reg, base_frame + slot32(1), ptrs),
                10 => {
                    // PUSH_MACHFRAME: rip, cs, rflags, rsp, ss (after an
                    // error code if info is 1).
                    let at = rsp + if op_info != 0 { 8 } else { 0 };
                    ctx.set(CTX_RIP, stack64(at));
                    ctx.set(CTX_RSP, stack64(at + 24));
                    machine_frame = true;
                }
                _ => {} // EPILOG (version 2) and spare codes: nothing to undo.
            }
            i += slots;
        }
        let after_codes = info + 4 + 2 * ((count + 1) & !1);
        if flags & UNW_FLAG_CHAININFO != 0 {
            // SAFETY: the chained RUNTIME_FUNCTION follows the codes.
            let chained = unsafe { *(after_codes as *const RuntimeFunction) };
            info = base + chained.unwind as usize;
            primary = false;
            continue;
        }
        if !machine_frame {
            let rsp = ctx.get(CTX_RSP);
            ctx.set(CTX_RIP, stack64(rsp));
            ctx.set(CTX_RSP, rsp + 8);
        }
        if flags & kind & (UNW_FLAG_EHANDLER | UNW_FLAG_UHANDLER) != 0 && !in_prologue {
            return (base + rd32(after_codes) as usize, after_codes as u64 + 4, frame);
        }
        return (0, 0, frame);
    }
}

/// RtlVirtualUnwind(HandlerType, ImageBase, ControlPc, FunctionEntry,
/// ContextRecord, *HandlerData, *EstablisherFrame, ContextPointers):
/// undoes the frame of the function at `pc` in `context`; returns its
/// language handler if it has one of `kind` and the pc is in its body.
///
/// # Safety
/// `entry` must be the function's RUNTIME_FUNCTION, `context` a CONTEXT,
/// the outputs writable (`pointers` may be null).
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub unsafe extern "system" fn RtlVirtualUnwind(
    kind: u32,
    image_base: u64,
    pc: u64,
    entry: *const RuntimeFunction,
    context: *mut u8,
    handler_data: *mut u64,
    establisher_frame: *mut u64,
    pointers: *mut u64,
) -> usize {
    let mut ctx = Context::from_ptr(context);
    let (handler, data, frame) = virtual_unwind(
        kind,
        image_base as usize,
        pc as usize,
        entry,
        &mut ctx,
        &Pointers(pointers),
    );
    // SAFETY: the caller's CONTEXT and outputs.
    unsafe {
        core::ptr::copy_nonoverlapping(ctx.0.as_ptr(), context, CONTEXT_SIZE);
        if !handler_data.is_null() {
            *handler_data = data;
        }
        if !establisher_frame.is_null() {
            *establisher_frame = frame;
        }
    }
    handler
}

/// Undoes one frame of `ctx`: through its unwind data, or as a leaf
/// function (return address at rsp) if it has none. Returns the language
/// handler of `kind`, its data and the establisher frame, with the image
/// base and entry.
fn unwind_frame(kind: u32, ctx: &mut Context) -> (usize, u64, u64, u64, *const RuntimeFunction) {
    let pc = ctx.get(CTX_RIP);
    let (base, entry) = lookup(pc);
    if entry.is_null() {
        let rsp = ctx.get(CTX_RSP);
        ctx.set(CTX_RIP, stack64(rsp));
        ctx.set(CTX_RSP, rsp + 8);
        return (0, 0, rsp, 0, entry);
    }
    let (h, data, frame) = virtual_unwind(
        kind,
        base as usize,
        pc as usize,
        entry,
        ctx,
        &Pointers(core::ptr::null_mut()),
    );
    (h, data, frame, base, entry)
}

/// The current thread's stack: (limit, base) from the TEB.
fn stack_bounds() -> (u64, u64) {
    let (base, limit): (u64, u64);
    // SAFETY: gs:[8] and gs:[0x10] are NT_TIB.StackBase and StackLimit.
    unsafe {
        core::arch::asm!("mov {}, gs:[8]", out(reg) base, options(nostack, readonly));
        core::arch::asm!("mov {}, gs:[0x10]", out(reg) limit, options(nostack, readonly));
    }
    (limit, base)
}

/// Whether `frame` can be a frame of this thread's stack: 8-byte aligned,
/// below the base (the limit only grows downwards, so it is not checked:
/// a frame in a region the guard has not reached yet is still valid).
fn frame_ok(frame: u64, stack_base: u64) -> bool {
    frame & 7 == 0 && frame < stack_base && frame != 0
}

// --- Stack back traces -------------------------------------------------

unsafe extern "system" {
    /// The assembly RtlCaptureContext below.
    fn RtlCaptureContext(context: *mut u8);
}

/// RtlCaptureStackBackTrace(FramesToSkip, FramesToCapture, BackTrace,
/// BackTraceHash): the return addresses of the frames above this one,
/// the caller's first, after skipping `skip`; the hash is their sum
/// (truncated). Returns how many were captured.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "system" fn RtlCaptureStackBackTrace(
    skip: u32,
    count: u32,
    frames: *mut usize,
    hash: *mut u32,
) -> u16 {
    let mut ctx = Context([0; CONTEXT_SIZE]);
    // SAFETY: a 16-byte aligned CONTEXT of ours.
    unsafe { RtlCaptureContext(ctx.0.as_mut_ptr()) };
    let (_, stack_base) = stack_bounds();
    let (mut n, mut skipped, mut sum) = (0u32, 0u32, 0u32);
    // The captured context is in this function; each step undoes a frame.
    for _ in 0..1024 {
        if n >= count.min(u32::from(u16::MAX)) {
            break;
        }
        unwind_frame(0, &mut ctx);
        let (pc, rsp) = (ctx.get(CTX_RIP), ctx.get(CTX_RSP));
        if pc == 0 || rsp >= stack_base {
            break;
        }
        if skipped < skip {
            skipped += 1;
            continue;
        }
        // SAFETY: the caller's array of `count` entries.
        unsafe { *frames.add(n as usize) = pc as usize };
        sum = sum.wrapping_add(pc as u32);
        n += 1;
    }
    if !hash.is_null() {
        // SAFETY: the caller's output.
        unsafe { *hash = sum };
    }
    n as u16
}

// --- Dispatch ----------------------------------------------------------

/// Calls the exception handlers of the frames from `context` outwards;
/// true if one returned ExceptionContinueExecution (the caller resumes
/// `context`). Never inlined: its CONTEXT copies must not enlarge the
/// dispatcher's own frame, which runs on what is left of an overflowed
/// stack (the vectored handlers get a stack overflow first).
#[inline(never)]
fn dispatch_frames(record: *mut u8, context: *mut u8) -> bool {
    let (_, stack_base) = stack_bounds();
    let mut ctx = Context::from_ptr(context);
    for _ in 0..4096 {
        let frame_ctx = ctx;
        let (handler, data, frame, base, entry) = unwind_frame(UNW_FLAG_EHANDLER, &mut ctx);
        if !frame_ok(frame, stack_base) {
            break;
        }
        if handler != 0 {
            let mut frame_ctx = frame_ctx;
            let mut dc = DispatcherContext {
                control_pc: frame_ctx.get(CTX_RIP),
                image_base: base,
                function_entry: entry,
                establisher_frame: frame,
                target_ip: 0,
                context_record: frame_ctx.0.as_mut_ptr(),
                language_handler: handler,
                handler_data: data,
                history_table: core::ptr::null_mut(),
                scope_index: 0,
                fill0: 0,
            };
            // SAFETY: the image's unwind data names an EXCEPTION_ROUTINE.
            let routine: LanguageHandler = unsafe { core::mem::transmute(handler) };
            if routine(record, frame, context, &mut dc) == DISPOSITION_CONTINUE_EXECUTION {
                // SAFETY: the record's flags.
                let flags = unsafe { (record.add(ER_FLAGS) as *const u32).read_unaligned() };
                if flags & EXCEPTION_NONCONTINUABLE != 0 {
                    NtTerminateProcess(-1, STATUS_NONCONTINUABLE_EXCEPTION);
                }
                return true;
            }
        }
        let (rip, rsp) = (ctx.get(CTX_RIP), ctx.get(CTX_RSP));
        if rip == 0 || rsp >= stack_base {
            break;
        }
    }
    false
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
    let handled = call_handlers(&exception, &mut pointers) || dispatch_frames(record, context);
    if call_handlers(&continue_, &mut pointers) || handled {
        NtContinue(context, 0);
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

// --- Unwinding ---------------------------------------------------------

/// RtlUnwindEx's work, from the caller's context `start`.
extern "system" fn unwind_to(
    target_frame: u64,
    target_ip: u64,
    record: *mut u8,
    return_value: u64,
    start: *const u8,
) -> ! {
    let mut own_record = [0u8; RECORD_SIZE];
    let record = if record.is_null() {
        own_record[..4].copy_from_slice(&STATUS_UNWIND.to_le_bytes());
        own_record.as_mut_ptr()
    } else {
        record
    };
    let set_flags = |flags: u32| {
        // SAFETY: the record's flags.
        unsafe { (record.add(ER_FLAGS) as *mut u32).write_unaligned(flags) }
    };
    // SAFETY: as above.
    let flags = unsafe { (record.add(ER_FLAGS) as *const u32).read_unaligned() };
    let mut flags = flags | EXCEPTION_UNWINDING;
    if target_frame == 0 {
        flags |= EXCEPTION_EXIT_UNWIND;
    }
    let (_, stack_base) = stack_bounds();
    let mut ctx = Context::from_ptr(start);
    let mut resume = None;
    for _ in 0..4096 {
        let frame_ctx = ctx;
        let (handler, data, frame, base, entry) = unwind_frame(UNW_FLAG_UHANDLER, &mut ctx);
        if !frame_ok(frame, stack_base) {
            break;
        }
        if target_frame != 0 && frame > target_frame {
            NtTerminateProcess(-1, STATUS_INVALID_UNWIND_TARGET);
        }
        let is_target = frame == target_frame;
        if handler != 0 {
            let mut frame_ctx = frame_ctx;
            set_flags(flags | if is_target { EXCEPTION_TARGET_UNWIND } else { 0 });
            let mut dc = DispatcherContext {
                control_pc: frame_ctx.get(CTX_RIP),
                image_base: base,
                function_entry: entry,
                establisher_frame: frame,
                target_ip,
                context_record: frame_ctx.0.as_mut_ptr(),
                language_handler: handler,
                handler_data: data,
                history_table: core::ptr::null_mut(),
                scope_index: 0,
                fill0: 0,
            };
            // SAFETY: the image's unwind data names an EXCEPTION_ROUTINE.
            let routine: LanguageHandler = unsafe { core::mem::transmute(handler) };
            routine(record, frame, frame_ctx.0.as_mut_ptr(), &mut dc);
            set_flags(flags);
        }
        if is_target {
            resume = Some(frame_ctx);
            break;
        }
        let (rip, rsp) = (ctx.get(CTX_RIP), ctx.get(CTX_RSP));
        if rip == 0 || rsp >= stack_base {
            break;
        }
    }
    let Some(mut ctx) = resume else {
        // An exit unwind (no target) or a target not on the stack.
        let status = if target_frame == 0 { STATUS_UNWIND } else { STATUS_BAD_STACK };
        NtTerminateProcess(-1, status);
        loop {
            core::hint::spin_loop();
        }
    };
    ctx.set(CTX_RAX, return_value);
    ctx.set(CTX_RIP, target_ip);
    NtContinue(ctx.0.as_ptr(), 0);
    NtTerminateProcess(-1, STATUS_BAD_STACK);
    loop {
        core::hint::spin_loop();
    }
}

/// RtlRaiseException's work: dispatch the record raised with `context`
/// (its caller's registers); resume it if a handler continued execution.
extern "system" fn raise(record: *mut u8, context: *mut u8) -> ! {
    dispatch_exception(record, context)
}

/// RtlRestoreContext(ContextRecord, ExceptionRecord): resumes the context
/// (through NtContinue; the exception record, used for unwind
/// consolidation, is not supported yet).
#[unsafe(no_mangle)]
pub extern "system" fn RtlRestoreContext(context: *const u8, _record: *mut u8) {
    NtContinue(context, 0);
}

/// Where the kernel puts the EXCEPTION_RECORD relative to the CONTEXT at
/// rsp, and the machine frame (rip, cs, rflags, rsp, ss of the fault) it
/// leaves above the record: gala's dispatcher frame (`nt::process`).
const RECORD_OFFSET: usize = 0x4f0;
const MACHINE_FRAME_OFFSET: usize = 0x590;

// RtlCaptureContext(ContextRecord): the caller's registers, Rip = its
// return address, Rsp = its rsp after the return.
//
// RtlRaiseException(ExceptionRecord): captures the caller's context,
// sets ExceptionAddress to the return address, dispatches.
//
// RtlUnwindEx(TargetFrame, TargetIp, ExceptionRecord, ReturnValue,
// ContextRecord, HistoryTable) and RtlUnwind (the first four): capture
// the caller's context and unwind from it (`unwind_to`).
//
// KiUserExceptionDispatcher: the kernel resumes a thread here after a
// user-mode exception, with rsp pointing at the CONTEXT of the fault, the
// EXCEPTION_RECORD above it and a machine frame above that. Its unwind
// data restores the nonvolatile registers from that CONTEXT and the
// machine frame, so walks continue into the faulting function. The
// CONTEXT's home area (P1Home..P4Home) is the callee's shadow space.
global_asm!(
    ".text",
    ".globl RtlCaptureContext",
    ".def RtlCaptureContext; .scl 2; .type 32; .endef",
    "RtlCaptureContext:",
    ".seh_proc RtlCaptureContext",
    "pushfq",
    ".seh_stackalloc 8",
    ".seh_endprologue",
    "mov [rcx + 0x78], rax",
    "mov [rcx + 0x80], rcx",
    "mov [rcx + 0x88], rdx",
    "mov [rcx + 0x90], rbx",
    "mov [rcx + 0xa0], rbp",
    "mov [rcx + 0xa8], rsi",
    "mov [rcx + 0xb0], rdi",
    "mov [rcx + 0xb8], r8",
    "mov [rcx + 0xc0], r9",
    "mov [rcx + 0xc8], r10",
    "mov [rcx + 0xd0], r11",
    "mov [rcx + 0xd8], r12",
    "mov [rcx + 0xe0], r13",
    "mov [rcx + 0xe8], r14",
    "mov [rcx + 0xf0], r15",
    "lea rax, [rsp + 0x10]",
    "mov [rcx + 0x98], rax",
    "mov rax, [rsp + 8]",
    "mov [rcx + 0xf8], rax",
    "mov eax, [rsp]",
    "mov [rcx + 0x44], eax",
    "mov word ptr [rcx + 0x38], cs",
    "mov word ptr [rcx + 0x3a], ds",
    "mov word ptr [rcx + 0x3c], es",
    "mov word ptr [rcx + 0x3e], fs",
    "mov word ptr [rcx + 0x40], gs",
    "mov word ptr [rcx + 0x42], ss",
    "fxsave [rcx + 0x100]",
    "stmxcsr [rcx + 0x34]",
    // CONTEXT_CONTROL | CONTEXT_INTEGER | CONTEXT_SEGMENTS |
    // CONTEXT_FLOATING_POINT
    "mov dword ptr [rcx + 0x30], 0x10000f",
    "mov rax, [rcx + 0x78]",
    "add rsp, 8",
    "ret",
    ".seh_endproc",
    "",
    ".globl RtlRaiseException",
    ".def RtlRaiseException; .scl 2; .type 32; .endef",
    "RtlRaiseException:",
    ".seh_proc RtlRaiseException",
    "sub rsp, 0x4f8",
    ".seh_stackalloc 0x4f8",
    ".seh_endprologue",
    "mov [rsp + 0x500], rcx",
    "lea rcx, [rsp + 0x20]",
    "call RtlCaptureContext",
    "mov rax, [rsp + 0x4f8]",
    "mov [rsp + 0x20 + 0xf8], rax",
    "mov rcx, [rsp + 0x500]",
    "mov [rcx + 0x10], rax",
    "lea rax, [rsp + 0x500]",
    "mov [rsp + 0x20 + 0x98], rax",
    "lea rdx, [rsp + 0x20]",
    "call {raise}",
    "int3",
    ".seh_endproc",
    "",
    ".globl RtlUnwindEx",
    ".def RtlUnwindEx; .scl 2; .type 32; .endef",
    "RtlUnwindEx:",
    ".seh_proc RtlUnwindEx",
    "sub rsp, 0x508",
    ".seh_stackalloc 0x508",
    ".seh_endprologue",
    "mov [rsp + 0x510], rcx",
    "mov [rsp + 0x518], rdx",
    "mov [rsp + 0x520], r8",
    "mov [rsp + 0x528], r9",
    "lea rcx, [rsp + 0x30]",
    "call RtlCaptureContext",
    "mov rax, [rsp + 0x508]",
    "mov [rsp + 0x30 + 0xf8], rax",
    "lea rax, [rsp + 0x510]",
    "mov [rsp + 0x30 + 0x98], rax",
    "mov rcx, [rsp + 0x510]",
    "mov rdx, [rsp + 0x518]",
    "mov r8, [rsp + 0x520]",
    "mov r9, [rsp + 0x528]",
    "lea rax, [rsp + 0x30]",
    "mov [rsp + 0x20], rax",
    "call {unwind_to}",
    "int3",
    ".seh_endproc",
    "",
    ".globl RtlUnwind",
    ".def RtlUnwind; .scl 2; .type 32; .endef",
    "RtlUnwind:",
    "jmp RtlUnwindEx",
    "",
    ".globl KiUserExceptionDispatcher",
    ".def KiUserExceptionDispatcher; .scl 2; .type 32; .endef",
    "KiUserExceptionDispatcher:",
    ".seh_proc KiUserExceptionDispatcher",
    ".seh_pushframe",
    ".seh_stackalloc {machine_frame}",
    ".seh_savereg rbx, 0x90",
    ".seh_savereg rbp, 0xa0",
    ".seh_savereg rsi, 0xa8",
    ".seh_savereg rdi, 0xb0",
    ".seh_savereg r12, 0xd8",
    ".seh_savereg r13, 0xe0",
    ".seh_savereg r14, 0xe8",
    ".seh_savereg r15, 0xf0",
    ".seh_savexmm xmm6, 0x200",
    ".seh_savexmm xmm7, 0x210",
    ".seh_savexmm xmm8, 0x220",
    ".seh_savexmm xmm9, 0x230",
    ".seh_savexmm xmm10, 0x240",
    ".seh_savexmm xmm11, 0x250",
    ".seh_savexmm xmm12, 0x260",
    ".seh_savexmm xmm13, 0x270",
    ".seh_savexmm xmm14, 0x280",
    ".seh_savexmm xmm15, 0x290",
    ".seh_endprologue",
    "cld",
    "mov rdx, rsp",
    "lea rcx, [rsp + {record}]",
    "call {dispatch}",
    "int3",
    ".seh_endproc",
    raise = sym raise,
    unwind_to = sym unwind_to,
    dispatch = sym dispatch_exception,
    record = const RECORD_OFFSET,
    machine_frame = const MACHINE_FRAME_OFFSET,
);
