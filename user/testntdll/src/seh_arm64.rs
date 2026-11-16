//! Frame-based exception handling on ARM64 (the part shared with x64 is
//! `dispatch.rs`), written from Microsoft's "ARM64 exception handling"
//! (the .pdata and .xdata records, packed unwind data and its canonical
//! prologue, the unwind codes, partial prologues and epilogues), the ARM64
//! CONTEXT's public layout (winnt.h: ARM64_NT_CONTEXT,
//! DISPATCHER_CONTEXT_ARM64) and the documented contracts of the Rtl*
//! functions; where the document leaves a choice open (the establisher
//! frame is the sp the unwind ends with; the handler is reported only from
//! a function's body), as Wine's arm64 unwinder does
//! (`dlls/ntdll/unwind.c`, read as a description of Windows' behaviour).
//!
//! - `RtlLookupFunctionEntry` (`ftable.rs`, shared with x64) finds the
//!   8-byte RUNTIME_FUNCTION covering a pc in its image's exception
//!   directory or in a table registered at run time.
//! - `RtlVirtualUnwind` undoes one frame in a CONTEXT: packed unwind data
//!   is expanded into the unwind codes of its canonical prologue and
//!   epilogue; a pc inside a prologue undoes only the instructions that
//!   ran, one inside an epilogue only those that had not; then the pc is
//!   the link register, unless a custom-stack code (a machine frame, a
//!   CONTEXT) restored it.
//! - Dispatch (`dispatch_frames`, after the vectored handlers) walks the
//!   frames from the exception's context and calls each one's handler
//!   until one returns ExceptionContinueExecution; `RtlUnwindEx` walks to
//!   the target frame calling them with EXCEPTION_UNWINDING, then resumes
//!   it at TargetPc with x0 = ReturnValue (after the consolidation
//!   callback, if the record asks for one).
//! - `KiUserExceptionDispatcher`: the kernel resumes a thread here after a
//!   user-mode exception with sp at the CONTEXT of the fault and the
//!   EXCEPTION_RECORD right above it (`kernel/src/nt/abi/arm64`'s frame);
//!   its unwind data (MSFT_OP_CONTEXT) leads a walk into the faulting
//!   function.
//! - `RtlCaptureContext`, `RtlRaiseException`, `RtlUnwindEx` and
//!   `RtlUnwind` start from their caller's registers.
//!
//! `.pdata` link records (Flag 3): a jump island's entry names its
//! parent's (the RVA in its upper 30 bits); the lookup returns the parent,
//! and a pc outside the parent's range unwinds as its body (the island
//! is part of it). An island's entry has no length: it reaches the next
//! entry (the last one, ISLAND_MAX bytes).
//!
//! Not yet: SVE state (alloc_z, save_zreg, save_preg), trap frames
//! (MSFT_OP_TRAP_FRAME) and x64 contexts (MSFT_OP_EC_CONTEXT); a frame
//! that needs one ends the walk, which the ledger records. Nested
//! exceptions and collided unwinds, as on x64.

use core::arch::global_asm;

use crate::ftable::lookup;
use crate::{NtContinue, NtTerminateProcess, rd32};

/// The ARM64 CONTEXT (ARM64_NT_CONTEXT): ContextFlags, Cpsr, X0-X28, Fp,
/// Lr, Sp, Pc, V0-V31, Fpcr, Fpsr, the debug registers.
pub(crate) const CTX_FLAGS: usize = 0;
const CONTEXT_SIZE: usize = 0x390;
const CTX_X0: usize = 0x08;
const CTX_LR: usize = 0xf8;
const CTX_SP: usize = 0x100;
const CTX_PC: usize = 0x108;
const CTX_V0: usize = 0x110;
const CONTEXT_UNWOUND_TO_CALL: u32 = 0x2000_0000;
/// RECORD's place in the kernel's dispatcher frame: right above the
/// CONTEXT.
const RECORD_OFFSET: usize = CONTEXT_SIZE;

// EXCEPTION_RECORD (the 64-bit layout).
const ER_FLAGS: usize = 0x04;
const RECORD_SIZE: usize = 0x98;
const EXCEPTION_NONCONTINUABLE: u32 = 1;
const EXCEPTION_UNWINDING: u32 = 2;
const EXCEPTION_EXIT_UNWIND: u32 = 4;
const EXCEPTION_TARGET_UNWIND: u32 = 0x20;
const EXCEPTION_SOFTWARE_ORIGINATE: u32 = 0x80;
const DISPOSITION_CONTINUE_EXECUTION: u32 = 0;
const STATUS_NONCONTINUABLE_EXCEPTION: u32 = 0xc000_0025;
const STATUS_INVALID_UNWIND_TARGET: u32 = 0xc000_0029;
const STATUS_BAD_STACK: u32 = 0xc000_0028;
const STATUS_UNWIND: u32 = 0xc000_0027;
const STATUS_UNWIND_CONSOLIDATE: u32 = 0x8000_0029;

/// IMAGE_ARM64_RUNTIME_FUNCTION_ENTRY: the function's RVA, then its
/// unwind data (an .xdata RVA, or packed: the low two bits say which).
#[repr(C)]
pub struct RuntimeFunction {
    begin: u32,
    unwind: u32,
}

/// A CONTEXT of ours, 16-byte aligned.
#[derive(Clone, Copy)]
#[repr(C, align(16))]
struct Context([u8; CONTEXT_SIZE]);

impl Context {
    fn from_ptr(p: *const u8) -> Context {
        let mut c = Context([0; CONTEXT_SIZE]);
        // SAFETY: the caller's CONTEXT, CONTEXT_SIZE bytes.
        unsafe { core::ptr::copy_nonoverlapping(p, c.0.as_mut_ptr(), CONTEXT_SIZE) };
        c
    }

    fn get(&self, off: usize) -> u64 {
        u64::from_le_bytes(self.0[off..off + 8].try_into().unwrap_or([0; 8]))
    }

    fn set(&mut self, off: usize, v: u64) {
        self.0[off..off + 8].copy_from_slice(&v.to_le_bytes());
    }

    fn flags(&self) -> u32 {
        u32::from_le_bytes(self.0[CTX_FLAGS..CTX_FLAGS + 4].try_into().unwrap_or([0; 4]))
    }

    fn set_flags(&mut self, flags: u32) {
        self.0[CTX_FLAGS..CTX_FLAGS + 4].copy_from_slice(&flags.to_le_bytes());
    }

    fn unwound(&self) -> bool {
        self.flags() & CONTEXT_UNWOUND_TO_CALL != 0
    }

    fn set_unwound(&mut self, on: bool) {
        let f = self.flags();
        self.set_flags(if on { f | CONTEXT_UNWOUND_TO_CALL } else { f & !CONTEXT_UNWOUND_TO_CALL });
    }

    /// X`n` (0-30; 29 is Fp and 30 Lr, which follow X28 in the layout).
    fn x(&self, n: usize) -> u64 {
        self.get(CTX_X0 + 8 * n)
    }

    fn set_x(&mut self, n: usize, v: u64) {
        self.set(CTX_X0 + 8 * n, v);
    }

    /// The low 64 bits of V`n` (D`n`).
    fn set_d(&mut self, n: usize, v: u64) {
        self.set(CTX_V0 + 16 * n, v);
    }

    /// V`n`, both halves.
    fn set_q(&mut self, n: usize, lo: u64, hi: u64) {
        self.set(CTX_V0 + 16 * n, lo);
        self.set(CTX_V0 + 16 * n + 8, hi);
    }
}

fn rd64(addr: u64) -> u64 {
    // SAFETY: a stack slot the unwind data says holds a saved value.
    unsafe { core::ptr::read_unaligned(addr as *const u64) }
}

// --- Lookup ------------------------------------------------------------

/// The length in bytes of the function `f` describes (0: a link record,
/// whose extent the table gives: `search`).
fn function_length(base: usize, f: &RuntimeFunction) -> u32 {
    match f.unwind & 3 {
        1 | 2 => ((f.unwind >> 2) & 0x7ff) * 4,
        0 => (rd32(base + f.unwind as usize) & 0x3_ffff) * 4,
        _ => 0,
    }
}

/// A link record (Flag 3: a jump island).
fn is_link(f: &RuntimeFunction) -> bool {
    f.unwind & 3 == 3
}

/// How far the last entry's jump island reaches, when it is a link
/// record (no next entry ends it): a branch through a register needs at
/// most four instructions (`adrp`, `add`, `br`, and a pad).
const ISLAND_MAX: u32 = 16;

/// How many link records `resolve` follows before giving up (a cycle in
/// a bad table).
const MAX_LINKS: usize = 8;

// The tables themselves (an image's, and those registered at run time)
// are `ftable.rs`'s, shared with x64; these are ARM64's entry format.

/// sizeof(IMAGE_ARM64_RUNTIME_FUNCTION_ENTRY).
pub(crate) const ENTRY_SIZE: usize = 8;

/// The entry of `table` (`count` entries, sorted, relative to `base`)
/// covering `rva`.
pub(crate) fn search(
    base: u64,
    table: *const RuntimeFunction,
    count: usize,
    rva: u32,
) -> *const RuntimeFunction {
    let (mut lo, mut hi) = (0usize, count);
    while lo < hi {
        let mid = (lo + hi) / 2;
        // SAFETY: mid < count entries of the table.
        let f = unsafe { &*table.add(mid) };
        // A jump island reaches the next entry.
        let end = if is_link(f) {
            // SAFETY: mid + 1 < count entries of the table.
            let next = (mid + 1 < count).then(|| unsafe { (*table.add(mid + 1)).begin });
            next.unwrap_or(f.begin.saturating_add(ISLAND_MAX))
        } else {
            f.begin + function_length(base as usize, f)
        };
        if rva < f.begin {
            hi = mid;
        } else if rva >= end {
            lo = mid + 1;
        } else {
            return f;
        }
    }
    core::ptr::null()
}

/// The entry the lookup returns for `f`: itself, or for a link record
/// its parent's (followed while the parent is a link too, at most
/// MAX_LINKS times; a longer chain is a bad table: no entry).
pub(crate) fn resolve(base: u64, f: *const RuntimeFunction) -> *const RuntimeFunction {
    let mut f = f;
    for _ in 0..=MAX_LINKS {
        // SAFETY: an entry of a table, or a parent a link record names in
        // the same image.
        let entry = unsafe { &*f };
        if !is_link(entry) {
            return f;
        }
        f = (base as usize + (entry.unwind & !3) as usize) as *const RuntimeFunction;
    }
    core::ptr::null()
}

// --- Unwind codes ------------------------------------------------------

/// The length in bytes of the unwind code starting with `b`.
fn code_len(b: u8) -> usize {
    match b {
        0x00..=0xbf => 1,
        0xc0..=0xdf => 2,
        0xe0 => 4,
        0xe2 => 2,
        0xe7 => 3,
        0xf8 => 2,
        0xf9 => 3,
        0xfa => 4,
        0xfb => 5,
        _ => 1,
    }
}

/// How many instructions the sequence at `codes` describes: its codes up
/// to `end` or `end_c`, the custom-stack ones (0xe8-0xef, no instruction)
/// not counted.
fn sequence_len(codes: &[u8]) -> usize {
    let (mut i, mut n) = (0, 0);
    while i < codes.len() && codes[i] != 0xe4 && codes[i] != 0xe5 {
        if codes[i] & 0xf8 != 0xe8 {
            n += 1;
        }
        i += code_len(codes[i]);
    }
    n
}

/// Why a frame could not be unwound.
#[derive(Clone, Copy, Debug)]
enum UnwindError {
    /// A code this unwinder does not handle (its first byte).
    Unsupported(u8),
}

/// Restores `count` X registers from `reg` on, saved at sp + 8 * `pos`
/// (`pos` < 0: a pre-indexed save, the slots at sp and sp moving up by
/// -8 * `pos` after them).
fn restore_x(ctx: &mut Context, reg: usize, count: usize, pos: i64) {
    let sp = ctx.get(CTX_SP);
    let at = sp + 8 * pos.max(0) as u64;
    for i in 0..count {
        ctx.set_x(reg + i, rd64(at + 8 * i as u64));
    }
    if pos < 0 {
        ctx.set(CTX_SP, sp + 8 * (-pos) as u64);
    }
}

/// As `restore_x` for D registers.
fn restore_d(ctx: &mut Context, reg: usize, count: usize, pos: i64) {
    let sp = ctx.get(CTX_SP);
    let at = sp + 8 * pos.max(0) as u64;
    for i in 0..count {
        ctx.set_d(reg + i, rd64(at + 8 * i as u64));
    }
    if pos < 0 {
        ctx.set(CTX_SP, sp + 8 * (-pos) as u64);
    }
}

/// As `restore_x` for Q registers (16-byte slots; `pos` in 16-byte units).
fn restore_q(ctx: &mut Context, reg: usize, count: usize, pos: i64) {
    let sp = ctx.get(CTX_SP);
    let at = sp + 16 * pos.max(0) as u64;
    for i in 0..count {
        let slot = at + 16 * i as u64;
        ctx.set_q(reg + i, rd64(slot), rd64(slot + 8));
    }
    if pos < 0 {
        ctx.set(CTX_SP, sp + 16 * (-pos) as u64);
    }
}

/// Removes a pointer authentication code from `lr` (pac_sign_lr's undo).
fn strip_pac(lr: u64) -> u64 {
    let out: u64;
    // SAFETY: XPACLRI (a hint: a no-op where pointer authentication is
    // absent) only changes x30, which is ours here.
    unsafe {
        core::arch::asm!(
            "mov x30, {lr}",
            "hint #7",
            "mov {out}, x30",
            lr = in(reg) lr,
            out = lateout(reg) out,
            out("x30") _,
            options(nomem, nostack, preserves_flags),
        )
    };
    out
}

/// Runs the unwind codes of `codes` on `ctx` after skipping the first
/// `skip` instructions' codes, up to `end`. Returns whether the pc is then
/// to come from the link register (no custom-stack code restored it).
fn run_codes(codes: &[u8], ctx: &mut Context, skip: usize) -> Result<bool, UnwindError> {
    let mut i = 0;
    let mut skipped = 0;
    while skipped < skip && i < codes.len() && codes[i] != 0xe4 {
        if codes[i] & 0xf8 != 0xe8 {
            skipped += 1;
        }
        i += code_len(codes[i]);
    }
    let mut pc_from_lr = true;
    // save_next: the count of registers the next pair code restores.
    let mut pair = 2usize;
    while i < codes.len() {
        let b = codes[i];
        let at = |k: usize| u64::from(codes.get(i + k).copied().unwrap_or(0));
        let val = (u64::from(b) << 8) | at(1);
        let mut next_pair = 2;
        match b {
            0x00..=0x1f => ctx.set(CTX_SP, ctx.get(CTX_SP) + 16 * u64::from(b & 0x1f)),
            0x20..=0x3f => restore_x(ctx, 19, pair, -i64::from(b & 0x1f)),
            0x40..=0x7f => restore_x(ctx, 29, 2, i64::from(b & 0x3f)),
            0x80..=0xbf => restore_x(ctx, 29, 2, -i64::from(b & 0x3f) - 1),
            0xc0..=0xc7 => ctx.set(CTX_SP, ctx.get(CTX_SP) + 16 * (val & 0x7ff)),
            0xc8..=0xcb => {
                restore_x(ctx, 19 + ((val >> 6) & 0xf) as usize, pair, (val & 0x3f) as i64)
            }
            0xcc..=0xcf => {
                restore_x(ctx, 19 + ((val >> 6) & 0xf) as usize, pair, -((val & 0x3f) as i64) - 1)
            }
            0xd0..=0xd3 => restore_x(ctx, 19 + ((val >> 6) & 0xf) as usize, 1, (val & 0x3f) as i64),
            0xd4..=0xd5 => {
                restore_x(ctx, 19 + ((val >> 5) & 0xf) as usize, 1, -((val & 0x1f) as i64) - 1)
            }
            0xd6..=0xd7 => {
                let z = (val & 0x3f) as i64;
                restore_x(ctx, 19 + 2 * ((val >> 6) & 0x7) as usize, 1, z);
                restore_x(ctx, 30, 1, z + 1);
            }
            0xd8..=0xd9 => {
                restore_d(ctx, 8 + ((val >> 6) & 0x7) as usize, pair, (val & 0x3f) as i64)
            }
            0xda..=0xdb => {
                restore_d(ctx, 8 + ((val >> 6) & 0x7) as usize, pair, -((val & 0x3f) as i64) - 1)
            }
            0xdc..=0xdd => restore_d(ctx, 8 + ((val >> 6) & 0x7) as usize, 1, (val & 0x3f) as i64),
            0xde => restore_d(ctx, 8 + ((val >> 5) & 0x7) as usize, 1, -((val & 0x1f) as i64) - 1),
            0xe0 => {
                let size = (at(1) << 16) | (at(2) << 8) | at(3);
                ctx.set(CTX_SP, ctx.get(CTX_SP) + 16 * size);
            }
            0xe1 => ctx.set(CTX_SP, ctx.x(29)),
            0xe2 => ctx.set(CTX_SP, ctx.x(29) - 8 * (val & 0xff)),
            0xe3 | 0xe5 => {}
            0xe4 => break,
            0xe6 => next_pair = pair + 2,
            0xe7 => {
                // save_any_reg: 0pxrrrrr, ttoooooo (tt: X, D, Q).
                let (r, o) = (at(1), at(2));
                let reg = (r & 0x1f) as usize;
                let paired = r & 0x40 != 0;
                let count = if paired { pair } else { 1 };
                let negative = r & 0x20 != 0;
                let off = (o & 0x3f) as i64;
                // Offsets count 16 bytes for a pair, a pre-indexed save or
                // a Q register, else 8.
                let units16 = paired || negative;
                match o >> 6 {
                    0 | 1 => {
                        let pos = if units16 { off * 2 } else { off };
                        let pos = if negative { -pos - 2 } else { pos };
                        if o >> 6 == 0 {
                            restore_x(ctx, reg, count, pos);
                        } else {
                            restore_d(ctx, reg, count, pos);
                        }
                    }
                    2 => restore_q(ctx, reg, count, if negative { -off - 1 } else { off }),
                    _ => return Err(UnwindError::Unsupported(b)),
                }
            }
            0xe9 => {
                // MSFT_OP_MACHINE_FRAME: the interrupted sp and pc.
                let sp = ctx.get(CTX_SP);
                ctx.set(CTX_PC, rd64(sp + 8));
                ctx.set(CTX_SP, rd64(sp));
                ctx.set_unwound(false);
                pc_from_lr = false;
            }
            0xea => {
                // MSFT_OP_CONTEXT: a whole CONTEXT at sp (the dispatcher's
                // frame); its own flags kept.
                let saved = Context::from_ptr(ctx.get(CTX_SP) as *const u8);
                let flags = ctx.flags();
                *ctx = saved;
                ctx.set_flags(
                    (flags & !CONTEXT_UNWOUND_TO_CALL) | (saved.flags() & CONTEXT_UNWOUND_TO_CALL),
                );
                pc_from_lr = false;
            }
            0xec => {
                // MSFT_OP_CLEAR_UNWOUND_TO_CALL.
                ctx.set(CTX_PC, ctx.get(CTX_LR));
                ctx.set_unwound(false);
                pc_from_lr = false;
            }
            0xfc => ctx.set(CTX_LR, strip_pac(ctx.get(CTX_LR))),
            _ => return Err(UnwindError::Unsupported(b)),
        }
        pair = next_pair;
        i += code_len(b);
    }
    Ok(pc_from_lr)
}

// --- Packed unwind data ------------------------------------------------

/// Unwind codes, built up for a packed function.
struct Codes {
    bytes: [u8; 64],
    len: usize,
    /// How many instructions they describe.
    count: usize,
}

impl Codes {
    fn new() -> Codes {
        Codes { bytes: [0; 64], len: 0, count: 0 }
    }

    fn push(&mut self, code: &[u8]) {
        for &b in code {
            if self.len < self.bytes.len() {
                self.bytes[self.len] = b;
                self.len += 1;
            }
        }
        self.count += 1;
    }

    /// A stack allocation of `size` bytes: alloc_s or alloc_m.
    fn alloc(&mut self, size: u32) {
        let units = size / 16;
        if units < 32 {
            self.push(&[units as u8]);
        } else {
            self.push(&[0xc0 | ((units >> 8) & 7) as u8, units as u8]);
        }
    }

    /// The codes in reverse order (unwind order), then `end`.
    fn reversed_with_end(&self, out: &mut Codes) {
        // Walk the codes forward to find each one's start, then emit them
        // last first.
        let mut starts = [0usize; 32];
        let (mut i, mut n) = (0, 0);
        while i < self.len && n < starts.len() {
            starts[n] = i;
            n += 1;
            i += code_len(self.bytes[i]);
        }
        for k in (0..n).rev() {
            let s = starts[k];
            let l = code_len(self.bytes[s]);
            out.push(&self.bytes[s..s + l]);
        }
        out.push(&[0xe4]);
        out.count -= 1; // `end` is not a prologue instruction
    }
}

/// The prologue's and the epilogue's unwind codes of a packed function
/// (Microsoft's canonical prologue, steps 1 to 6, in unwind order), and
/// the epilogue's instruction count (its `ret` included).
fn packed_codes(unwind: u32) -> (Codes, Codes) {
    let reg_f = (unwind >> 13) & 7;
    let reg_i = (unwind >> 16) & 0xf;
    let h = (unwind >> 20) & 1;
    let cr = (unwind >> 21) & 3;
    let frame = ((unwind >> 23) & 0x1ff) * 16;
    // Step 0: the areas' sizes.
    let int_size = reg_i * 8 + if cr == 1 { 8 } else { 0 };
    let fp_size = reg_f * 8 + if reg_f != 0 { 8 } else { 0 };
    let mut save_size = (int_size + fp_size + 64 * h + 0xf) & !0xf;
    // With nothing else saved, the home area is part of the frame and its
    // stores are no prologue step of their own.
    let homes = h != 0 && !(reg_i == 0 && reg_f == 0 && cr != 1);
    if h != 0 && !homes {
        save_size = 0;
    }
    let local = frame.saturating_sub(save_size);
    // The prologue in execution order: one code per instruction; `epi`
    // marks the ones the epilogue mirrors (not the homing stores, not
    // set_fp).
    let mut pro = Codes::new();
    let mut epi = Codes::new();
    let both = |c: &[u8], pro: &mut Codes, epi: &mut Codes| {
        pro.push(c);
        epi.push(c);
    };
    // Step 1.
    if cr == 2 {
        both(&[0xfc], &mut pro, &mut epi);
    }
    // Step 2: x19.. in pairs, the first store pre-indexed by the save
    // area; an odd last register single, or paired with lr (CR 1).
    let mut first = true;
    let mut k = 0;
    while k + 1 < reg_i {
        let x = k; // x(19 + x)
        if first {
            let z = save_size / 8 - 1;
            both(&[0xcc | (x >> 2) as u8, (((x & 3) << 6) | z) as u8], &mut pro, &mut epi);
            first = false;
        } else {
            both(&[0xc8 | (x >> 2) as u8, (((x & 3) << 6) | k) as u8], &mut pro, &mut epi);
        }
        k += 2;
    }
    if reg_i % 2 == 1 {
        let x = reg_i - 1;
        if cr == 1 {
            if first {
                // A pre-indexed register-and-lr pair has no unwind code:
                // the save area is allocated first.
                pro.alloc(save_size);
                epi.alloc(save_size);
                first = false;
                both(
                    &[0xd6 | ((x / 2) >> 2) as u8, (((x / 2) & 3) << 6) as u8],
                    &mut pro,
                    &mut epi,
                );
            } else {
                both(
                    &[0xd6 | ((x / 2) >> 2) as u8, ((((x / 2) & 3) << 6) | x) as u8],
                    &mut pro,
                    &mut epi,
                );
            }
        } else if first {
            let z = save_size / 8 - 1;
            both(&[0xd4 | (x >> 3) as u8, (((x & 7) << 5) | z) as u8], &mut pro, &mut epi);
            first = false;
        } else {
            both(&[0xd0 | (x >> 2) as u8, (((x & 3) << 6) | x) as u8], &mut pro, &mut epi);
        }
    }
    // Step 3: lr at the end of the integer area (an even RegI with CR 1).
    if cr == 1 && reg_i.is_multiple_of(2) {
        let x = 11; // x30 = x(19 + 11)
        if first {
            let z = save_size / 8 - 1;
            both(&[0xd4 | (x >> 3) as u8, (((x & 7) << 5) | z) as u8], &mut pro, &mut epi);
            first = false;
        } else {
            let z = int_size / 8 - 1;
            both(&[0xd0 | (x >> 2) as u8, (((x & 3) << 6) | z) as u8], &mut pro, &mut epi);
        }
    }
    // Step 4: d8.. in pairs from the end of the integer area, an odd last
    // one single.
    if reg_f != 0 {
        let n = reg_f + 1;
        let mut d = 0;
        while d + 1 < n {
            let z = (int_size + 8 * d) / 8;
            if first {
                let z = save_size / 8 - 1;
                both(&[0xda | (d >> 2) as u8, (((d & 3) << 6) | z) as u8], &mut pro, &mut epi);
                first = false;
            } else {
                both(&[0xd8 | (d >> 2) as u8, (((d & 3) << 6) | z) as u8], &mut pro, &mut epi);
            }
            d += 2;
        }
        if n % 2 == 1 {
            let z = (int_size + fp_size - 8) / 8;
            both(&[0xdc | (d >> 2) as u8, (((d & 3) << 6) | z) as u8], &mut pro, &mut epi);
        }
    }
    // Step 5: the home area (stores the epilogue does not undo).
    if homes {
        for _ in 0..4 {
            pro.push(&[0xe3]);
        }
    }
    // Step 6: the rest of the frame, with <x29,lr> at its bottom when
    // chained.
    if cr == 2 || cr == 3 {
        if local <= 512 {
            both(&[0x80 | (local / 8 - 1) as u8], &mut pro, &mut epi);
        } else {
            if local > 4080 {
                pro.alloc(4080);
                epi.alloc(4080);
                pro.alloc(local - 4080);
                epi.alloc(local - 4080);
            } else {
                pro.alloc(local);
                epi.alloc(local);
            }
            both(&[0x40], &mut pro, &mut epi);
        }
        pro.push(&[0xe1]);
    } else if local > 4080 {
        pro.alloc(4080);
        epi.alloc(4080);
        pro.alloc(local - 4080);
        epi.alloc(local - 4080);
    } else if local > 0 {
        pro.alloc(local);
        epi.alloc(local);
    }
    let (mut p, mut e) = (Codes::new(), Codes::new());
    pro.reversed_with_end(&mut p);
    epi.reversed_with_end(&mut e);
    (p, e)
}

/// Unwinds a function with packed unwind data at instruction `offset`.
fn unwind_packed(unwind: u32, offset: Option<u32>, ctx: &mut Context) -> Result<bool, UnwindError> {
    let (pro, epi) = packed_codes(unwind);
    let length = (unwind >> 2) & 0x7ff;
    // Flag 2: a fragment with no prologue or epilogue of its own; outside
    // the function (a jump island): the body.
    if let (1, Some(offset)) = (unwind & 3, offset) {
        let plen = pro.count as u32;
        if offset < plen {
            return run_codes(&pro.bytes[..pro.len], ctx, (plen - offset) as usize);
        }
        // The epilogue ends the function: its codes and the `ret`.
        let elen = epi.count as u32 + 1;
        if length >= elen && offset >= length - elen {
            return run_codes(&epi.bytes[..epi.len], ctx, (offset - (length - elen)) as usize);
        }
    }
    run_codes(&pro.bytes[..pro.len], ctx, 0)
}

// --- Full unwind data (.xdata) -----------------------------------------

/// What unwinding a frame found: the handler (0: none), its data and
/// whether the pc is then the link register.
struct Unwound {
    handler: usize,
    data: u64,
    pc_from_lr: bool,
}

/// Unwinds a function with an .xdata record at `xdata` at instruction
/// `offset`.
fn unwind_full(
    base: usize,
    xdata: usize,
    offset: Option<u32>,
    ctx: &mut Context,
) -> Result<Unwound, UnwindError> {
    let w0 = rd32(xdata);
    let length = w0 & 0x3_ffff;
    let has_handler = w0 & (1 << 20) != 0;
    let epilog_in_header = w0 & (1 << 21) != 0;
    let (mut epilogs, mut words) = ((w0 >> 22) & 0x1f, (w0 >> 27) & 0x1f);
    let mut at = xdata + 4;
    if epilogs == 0 && words == 0 {
        let w1 = rd32(at);
        epilogs = w1 & 0xffff;
        words = (w1 >> 16) & 0xff;
        at += 4;
    }
    let scopes = at;
    if !epilog_in_header {
        at += 4 * epilogs as usize;
    }
    // SAFETY: the record's unwind code bytes, `words` words long.
    let codes = unsafe { core::slice::from_raw_parts(at as *const u8, 4 * words as usize) };
    let none = |pc_from_lr| Ok(Unwound { handler: 0, data: 0, pc_from_lr });
    // Outside the function (a jump island, `offset` None): the body.
    if let Some(offset) = offset {
        // In the prologue: only what ran is undone.
        let plen = sequence_len(codes) as u32;
        if offset < plen {
            return none(run_codes(codes, ctx, (plen - offset) as usize)?);
        }
        // In an epilogue: only what had not run.
        if epilog_in_header {
            let index = epilogs as usize;
            if let Some(seq) = codes.get(index..) {
                let elen = sequence_len(seq) as u32 + 1;
                if length >= elen && offset >= length - elen {
                    return none(run_codes(seq, ctx, (offset - (length - elen)) as usize)?);
                }
            }
        } else {
            for s in 0..epilogs as usize {
                let scope = rd32(scopes + 4 * s);
                let start = scope & 0x3_ffff;
                let index = (scope >> 22) as usize;
                if offset < start {
                    break;
                }
                let Some(seq) = codes.get(index..) else { continue };
                if offset <= start + sequence_len(seq) as u32 {
                    return none(run_codes(seq, ctx, (offset - start) as usize)?);
                }
            }
        }
    }
    let pc_from_lr = run_codes(codes, ctx, 0)?;
    if has_handler {
        let handler_rva = at + 4 * words as usize;
        return Ok(Unwound {
            handler: base + rd32(handler_rva) as usize,
            data: (handler_rva + 4) as u64,
            pc_from_lr,
        });
    }
    none(pc_from_lr)
}

/// Undoes the frame of the function `f` (in the image at `base`) in
/// `ctx`, at `pc`: returns its handler and data. The establisher frame is
/// the sp it ends with.
fn virtual_unwind(
    base: usize,
    pc: u64,
    f: &RuntimeFunction,
    ctx: &mut Context,
) -> Result<Unwound, UnwindError> {
    ctx.set_unwound(true);
    let f = if is_link(f) {
        let parent = resolve(base as u64, f);
        if parent.is_null() {
            return Err(UnwindError::Unsupported(0));
        }
        // SAFETY: the parent entry `resolve` found.
        unsafe { &*parent }
    } else {
        f
    };
    // The instruction's offset in the function; None outside it (a jump
    // island, which unwinds as the body).
    let rva = (pc as usize - base) as u32;
    let offset = rva.checked_sub(f.begin).filter(|&d| d < function_length(base, f)).map(|d| d / 4);
    let unwound = match f.unwind & 3 {
        1 | 2 => Unwound { handler: 0, data: 0, pc_from_lr: unwind_packed(f.unwind, offset, ctx)? },
        _ => unwind_full(base, base + f.unwind as usize, offset, ctx)?,
    };
    if unwound.pc_from_lr {
        ctx.set(CTX_PC, ctx.get(CTX_LR));
    }
    Ok(unwound)
}

/// Says in the ledger that a frame could not be unwound (the walk ends).
fn report_unwind_error(e: UnwindError) {
    let UnwindError::Unsupported(code) = e;
    let what = b"ntdll.dll!ARM64 unwind code";
    crate::GalaRecordMissing(what.as_ptr(), what.len(), 2, u64::from(code));
}

/// RtlVirtualUnwind(HandlerType, ImageBase, ControlPc, FunctionEntry,
/// ContextRecord, *HandlerData, *EstablisherFrame, ContextPointers): undoes
/// one frame in the CONTEXT; returns its language handler (null in a
/// prologue or epilogue, or without one).
///
/// # Safety
/// The pointers must be valid; `function` must describe `pc`.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn RtlVirtualUnwind(
    _handler_type: u32,
    base: u64,
    pc: u64,
    function: *const RuntimeFunction,
    context: *mut u8,
    handler_data: *mut u64,
    establisher_frame: *mut u64,
    _pointers: *mut u8,
) -> usize {
    let mut ctx = Context::from_ptr(context);
    let (handler, data) = if function.is_null() {
        // A leaf: the return address is in lr.
        ctx.set(CTX_PC, ctx.get(CTX_LR));
        ctx.set_unwound(true);
        (0, 0)
    } else {
        // SAFETY: the caller's RUNTIME_FUNCTION.
        match virtual_unwind(base as usize, pc, unsafe { &*function }, &mut ctx) {
            Ok(u) => (u.handler, u.data),
            Err(e) => {
                report_unwind_error(e);
                (0, 0)
            }
        }
    };
    // SAFETY: the caller's CONTEXT and outputs.
    unsafe {
        core::ptr::copy_nonoverlapping(ctx.0.as_ptr(), context, CONTEXT_SIZE);
        if !handler_data.is_null() {
            *handler_data = data;
        }
        if !establisher_frame.is_null() {
            *establisher_frame = ctx.get(CTX_SP);
        }
    }
    handler
}

/// Undoes one frame of `ctx`: through its unwind data, or as a leaf (the
/// return address in lr). The pc of a frame reached by unwinding a call is
/// a return address, so the instruction before it is the one looked up.
/// Returns the handler, its data, the establisher frame, the image base and
/// the entry; None if the frame cannot be unwound.
fn unwind_frame(ctx: &mut Context) -> Option<(usize, u64, u64, u64, *const RuntimeFunction)> {
    let pc = ctx.get(CTX_PC);
    let lookup_pc = if ctx.unwound() { pc.wrapping_sub(4) } else { pc };
    let (base, entry) = lookup(lookup_pc);
    if entry.is_null() {
        ctx.set(CTX_PC, ctx.get(CTX_LR));
        ctx.set_unwound(true);
        return Some((0, 0, ctx.get(CTX_SP), 0, entry));
    }
    // SAFETY: an entry of the image's exception directory.
    match virtual_unwind(base as usize, lookup_pc, unsafe { &*entry }, ctx) {
        Ok(u) => Some((u.handler, u.data, ctx.get(CTX_SP), base, entry)),
        Err(e) => {
            report_unwind_error(e);
            None
        }
    }
}

/// The current thread's stack: (limit, base) from the TEB (x18).
fn stack_bounds() -> (u64, u64) {
    let (base, limit): (u64, u64);
    // SAFETY: [x18 + 8] and [x18 + 0x10] are NT_TIB.StackBase and
    // StackLimit.
    unsafe {
        core::arch::asm!("ldr {}, [x18, #8]", out(reg) base, options(nostack, readonly));
        core::arch::asm!("ldr {}, [x18, #0x10]", out(reg) limit, options(nostack, readonly));
    }
    (limit, base)
}

/// Whether `frame` can be a frame of this thread's stack: 16-byte aligned,
/// below the base.
fn frame_ok(frame: u64, stack_base: u64) -> bool {
    frame & 0xf == 0 && frame < stack_base && frame != 0
}

// --- Stack back traces -------------------------------------------------

unsafe extern "system" {
    /// The assembly RtlCaptureContext below.
    fn RtlCaptureContext(context: *mut u8);
}

/// RtlCaptureStackBackTrace(FramesToSkip, FramesToCapture, BackTrace,
/// BackTraceHash): the return addresses of the frames above this one, the
/// caller's first, after skipping `skip`; the hash is their sum
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
    // The captured Pc is the return address of that call, in this function.
    ctx.set_unwound(true);
    let (_, stack_base) = stack_bounds();
    let (mut n, mut skipped, mut sum) = (0u32, 0u32, 0u32);
    let mut last = (ctx.get(CTX_PC), ctx.get(CTX_SP));
    for _ in 0..1024 {
        if n >= count.min(u32::from(u16::MAX)) {
            break;
        }
        if unwind_frame(&mut ctx).is_none() {
            break;
        }
        let (pc, sp) = (ctx.get(CTX_PC), ctx.get(CTX_SP));
        // A frame without unwind data whose Lr is its own return address
        // (no progress) ends the walk as the stack's end does.
        if pc == 0 || sp >= stack_base || (pc, sp) == last {
            break;
        }
        last = (pc, sp);
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

/// DISPATCHER_CONTEXT (ARM64's: DISPATCHER_CONTEXT_ARM64).
#[repr(C)]
struct DispatcherContext {
    control_pc: u64,
    image_base: u64,
    function_entry: *const RuntimeFunction,
    establisher_frame: u64,
    target_pc: u64,
    context_record: *mut u8,
    language_handler: usize,
    handler_data: u64,
    history_table: *mut u8,
    scope_index: u32,
    control_pc_is_unwound: u8,
    fill0: [u8; 3],
    non_volatile_registers: *mut u8,
}

type LanguageHandler = extern "system" fn(*mut u8, u64, *mut u8, *mut DispatcherContext) -> u32;

/// Calls the handler `handler` of the frame `frame` as dispatch and
/// unwinding do; returns its disposition.
#[allow(clippy::too_many_arguments)]
fn call_handler(
    record: *mut u8,
    context: *mut u8,
    frame_ctx: &mut Context,
    (handler, data, frame, base, entry): (usize, u64, u64, u64, *const RuntimeFunction),
    target_pc: u64,
) -> u32 {
    let mut dc = DispatcherContext {
        control_pc: frame_ctx.get(CTX_PC),
        image_base: base,
        function_entry: entry,
        establisher_frame: frame,
        target_pc,
        context_record: frame_ctx.0.as_mut_ptr(),
        language_handler: handler,
        handler_data: data,
        history_table: core::ptr::null_mut(),
        scope_index: 0,
        control_pc_is_unwound: u8::from(frame_ctx.unwound()),
        fill0: [0; 3],
        non_volatile_registers: core::ptr::null_mut(),
    };
    // SAFETY: the image's unwind data names an EXCEPTION_ROUTINE.
    let routine: LanguageHandler = unsafe { core::mem::transmute(handler) };
    routine(record, frame, context, &mut dc)
}

// --- Dispatch ----------------------------------------------------------

/// Calls the exception handlers of the frames from `context` outwards;
/// true if one returned ExceptionContinueExecution (the caller resumes
/// `context`). Never inlined: its CONTEXT copies must not enlarge the
/// dispatcher's own frame.
#[inline(never)]
pub(crate) fn dispatch_frames(record: *mut u8, context: *mut u8) -> bool {
    let (_, stack_base) = stack_bounds();
    let mut ctx = Context::from_ptr(context);
    for _ in 0..4096 {
        let mut frame_ctx = ctx;
        let Some(found) = unwind_frame(&mut ctx) else { break };
        if !frame_ok(found.2, stack_base) {
            break;
        }
        if found.0 != 0
            && call_handler(record, context, &mut frame_ctx, found, 0)
                == DISPOSITION_CONTINUE_EXECUTION
        {
            // SAFETY: the record's flags.
            let flags = unsafe { (record.add(ER_FLAGS) as *const u32).read_unaligned() };
            if flags & EXCEPTION_NONCONTINUABLE != 0 {
                NtTerminateProcess(-1, STATUS_NONCONTINUABLE_EXCEPTION);
            }
            return true;
        }
        let (pc, sp) = (ctx.get(CTX_PC), ctx.get(CTX_SP));
        if pc == 0 || sp >= stack_base {
            break;
        }
    }
    false
}

// --- Unwinding ---------------------------------------------------------

/// RtlUnwindEx's work, from the caller's context `start`.
extern "system" fn unwind_to(
    target_frame: u64,
    target_pc: u64,
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
    // As x64's: the unwind's handlers do not see EXCEPTION_SOFTWARE_ORIGINATE.
    let mut flags = (flags & !EXCEPTION_SOFTWARE_ORIGINATE) | EXCEPTION_UNWINDING;
    if target_frame == 0 {
        flags |= EXCEPTION_EXIT_UNWIND;
    }
    let (_, stack_base) = stack_bounds();
    let mut ctx = Context::from_ptr(start);
    let mut resume = None;
    for _ in 0..4096 {
        let mut frame_ctx = ctx;
        let Some(found) = unwind_frame(&mut ctx) else { break };
        let frame = found.2;
        if !frame_ok(frame, stack_base) {
            break;
        }
        if target_frame != 0 && frame > target_frame {
            NtTerminateProcess(-1, STATUS_INVALID_UNWIND_TARGET);
        }
        let is_target = frame == target_frame;
        if found.0 != 0 {
            set_flags(flags | if is_target { EXCEPTION_TARGET_UNWIND } else { 0 });
            let mut handler_ctx = frame_ctx;
            call_handler(record, handler_ctx.0.as_mut_ptr(), &mut frame_ctx, found, target_pc);
            set_flags(flags);
        }
        if is_target {
            resume = Some(frame_ctx);
            break;
        }
        let (pc, sp) = (ctx.get(CTX_PC), ctx.get(CTX_SP));
        if pc == 0 || sp >= stack_base {
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
    ctx.set_x(0, return_value);
    ctx.set(CTX_PC, target_pc);
    consolidate_if_asked(ctx.0.as_ptr(), record);
    NtContinue(ctx.0.as_ptr(), 0);
    NtTerminateProcess(-1, STATUS_BAD_STACK);
    loop {
        core::hint::spin_loop();
    }
}

/// RtlRestoreContext(ContextRecord, ExceptionRecord): resumes the context
/// (through NtContinue), after the consolidation callback if the record
/// asks for one.
#[unsafe(no_mangle)]
pub extern "system" fn RtlRestoreContext(context: *const u8, record: *mut u8) {
    consolidate_if_asked(context, record);
    NtContinue(context, 0);
}

/// Unwind consolidation (STATUS_UNWIND_CONSOLIDATE, how MSVC runs C++
/// catch blocks): ExceptionInformation[0] is a callback taking the record
/// and returning where to continue. It runs on the current stack, below
/// the frames the unwind removed (they still hold the exception object),
/// in a frame whose unwind data (MSFT_OP_CONTEXT) leads straight to
/// `context` (so a walk from the callback reaches the target frame); then
/// `context` resumes at the address it returned.
fn consolidate_if_asked(context: *const u8, record: *mut u8) {
    if record.is_null() {
        return;
    }
    // SAFETY: an EXCEPTION_RECORD: code, then NumberParameters at +0x18
    // and ExceptionInformation at +0x20.
    let (code, count, callback) = unsafe {
        (
            (record as *const u32).read_unaligned(),
            (record.add(0x18) as *const u32).read_unaligned(),
            (record.add(0x20) as *const u64).read_unaligned(),
        )
    };
    if code != STATUS_UNWIND_CONSOLIDATE || count < 1 || callback == 0 {
        return;
    }
    let block = Context::from_ptr(context);
    // SAFETY: the block holds the CONTEXT; the callback is the record's;
    // gala_consolidate does not return.
    unsafe { gala_consolidate(block.0.as_ptr(), callback, record) }
}

unsafe extern "system" {
    /// Copies the CONTEXT `block` to a new frame below the current sp,
    /// calls `callback(record)` there (in a frame whose unwind data is that
    /// CONTEXT), sets its Pc to the result and resumes it.
    fn gala_consolidate(block: *const u8, callback: u64, record: *mut u8) -> !;
}

// RtlCaptureContext(x0 = ContextRecord): X0-X28, Fp, Lr, Sp (the
// caller's), Pc = Lr (the return address), Cpsr's condition flags, V0-V31,
// Fpcr and Fpsr; ContextFlags CONTEXT_FULL. Only x16 and x17 (the
// intra-procedure scratch registers) are used, after being stored. A leaf
// with no unwind data: a walk takes its return address from lr.
//
// RtlRaiseException(x0 = ExceptionRecord): a frame record and room for a
// CONTEXT; the captured context is then made the caller's (X0 the record,
// Fp and Lr the caller's, Sp above this frame, Pc the return address,
// CONTEXT_UNWOUND_TO_CALL), ExceptionAddress set to the return address,
// and `raise` dispatches.
//
// RtlUnwindEx(x0 TargetFrame, x1 TargetPc, x2 ExceptionRecord, x3
// ReturnValue, x4 ContextRecord, x5 HistoryTable) and RtlUnwind (the
// first four): capture the caller's context and unwind from it
// (`unwind_to`).
//
// KiUserExceptionDispatcher: sp is at the CONTEXT, the record above it;
// its unwind data (`.seh_context`) restores that CONTEXT, so walks
// continue into the faulting function.
//
// gala_consolidate(x0 block, x1 callback, x2 record): the CONTEXT copied to
// a frame of its own below the current sp, described the same way.
global_asm!(
    ".text",
    ".globl RtlCaptureContext",
    ".p2align 2",
    "RtlCaptureContext:",
    "stp x0, x1, [x0, #0x08]",
    "stp x2, x3, [x0, #0x18]",
    "stp x4, x5, [x0, #0x28]",
    "stp x6, x7, [x0, #0x38]",
    "stp x8, x9, [x0, #0x48]",
    "stp x10, x11, [x0, #0x58]",
    "stp x12, x13, [x0, #0x68]",
    "stp x14, x15, [x0, #0x78]",
    "stp x16, x17, [x0, #0x88]",
    "stp x18, x19, [x0, #0x98]",
    "stp x20, x21, [x0, #0xa8]",
    "stp x22, x23, [x0, #0xb8]",
    "stp x24, x25, [x0, #0xc8]",
    "stp x26, x27, [x0, #0xd8]",
    "str x28, [x0, #0xe8]",
    "stp x29, x30, [x0, #0xf0]",
    "mov x16, sp",
    "stp x16, x30, [x0, #0x100]",
    "mrs x16, nzcv",
    "str w16, [x0, #0x4]",
    "add x16, x0, #0x110",
    "stp q0, q1, [x16, #0x00]",
    "stp q2, q3, [x16, #0x20]",
    "stp q4, q5, [x16, #0x40]",
    "stp q6, q7, [x16, #0x60]",
    "stp q8, q9, [x16, #0x80]",
    "stp q10, q11, [x16, #0xa0]",
    "stp q12, q13, [x16, #0xc0]",
    "stp q14, q15, [x16, #0xe0]",
    "stp q16, q17, [x16, #0x100]",
    "stp q18, q19, [x16, #0x120]",
    "stp q20, q21, [x16, #0x140]",
    "stp q22, q23, [x16, #0x160]",
    "stp q24, q25, [x16, #0x180]",
    "stp q26, q27, [x16, #0x1a0]",
    "stp q28, q29, [x16, #0x1c0]",
    "stp q30, q31, [x16, #0x1e0]",
    "mrs x17, fpcr",
    "str w17, [x0, #0x310]",
    "mrs x17, fpsr",
    "str w17, [x0, #0x314]",
    // CONTEXT_ARM64 | CONTROL | INTEGER | FLOATING_POINT
    "mov w16, #0x7",
    "movk w16, #0x40, lsl #16",
    "str w16, [x0]",
    "ret",
    "",
    ".globl RtlRaiseException",
    ".p2align 2",
    "RtlRaiseException:",
    ".seh_proc RtlRaiseException",
    "stp x29, x30, [sp, #-16]!",
    ".seh_save_fplr_x 16",
    "mov x29, sp",
    ".seh_set_fp",
    "sub sp, sp, #0x3a0",
    ".seh_stackalloc 0x3a0",
    ".seh_endprologue",
    "str x0, [sp, #0x390]",
    "mov x0, sp",
    "bl RtlCaptureContext",
    "ldr x1, [sp, #0x390]",
    "str x1, [sp, #0x08]",
    "ldp x2, x3, [x29]",
    "stp x2, x3, [sp, #0xf0]",
    "add x4, x29, #16",
    "stp x4, x3, [sp, #0x100]",
    "ldr w5, [sp]",
    "orr w5, w5, #0x20000000",
    "str w5, [sp]",
    "str x3, [x1, #0x10]",
    "mov x0, x1",
    "mov x1, sp",
    "bl {raise}",
    "brk #0xf000",
    ".seh_endproc",
    "",
    ".globl RtlUnwindEx",
    ".p2align 2",
    "RtlUnwindEx:",
    ".seh_proc RtlUnwindEx",
    "stp x29, x30, [sp, #-16]!",
    ".seh_save_fplr_x 16",
    "mov x29, sp",
    ".seh_set_fp",
    // The four arguments at sp, the CONTEXT above them.
    "sub sp, sp, #0x3b0",
    ".seh_stackalloc 0x3b0",
    ".seh_endprologue",
    "stp x0, x1, [sp]",
    "stp x2, x3, [sp, #16]",
    "add x0, sp, #0x20",
    "bl RtlCaptureContext",
    "add x9, sp, #0x20",
    "ldp x2, x3, [x29]",
    "stp x2, x3, [x9, #0xf0]",
    "add x4, x29, #16",
    "stp x4, x3, [x9, #0x100]",
    "ldr w5, [x9]",
    "orr w5, w5, #0x20000000",
    "str w5, [x9]",
    "ldp x0, x1, [sp]",
    "ldp x2, x3, [sp, #16]",
    "mov x4, x9",
    "bl {unwind_to}",
    "brk #0xf000",
    ".seh_endproc",
    "",
    ".globl RtlUnwind",
    ".p2align 2",
    "RtlUnwind:",
    "b RtlUnwindEx",
    "",
    ".globl KiUserExceptionDispatcher",
    ".p2align 2",
    "KiUserExceptionDispatcher:",
    ".seh_proc KiUserExceptionDispatcher",
    ".seh_context",
    ".seh_endprologue",
    "add x0, sp, #{record}",
    "mov x1, sp",
    "bl {dispatch}",
    "brk #0xf000",
    ".seh_endproc",
    "",
    ".globl gala_consolidate",
    ".p2align 2",
    "gala_consolidate:",
    // A frame of CONTEXT_SIZE (16-byte aligned) below the current sp, the
    // CONTEXT copied into it, then the callback in the frame below.
    "mov x9, sp",
    "sub x9, x9, #{size}",
    "and x9, x9, #0xfffffffffffffff0",
    "mov x10, #0",
    "1:",
    "ldr x11, [x0, x10]",
    "str x11, [x9, x10]",
    "add x10, x10, #8",
    "cmp x10, #{size}",
    "b.lo 1b",
    "mov sp, x9",
    "b gala_consolidate_frame",
    ".p2align 2",
    "gala_consolidate_frame:",
    ".seh_proc gala_consolidate_frame",
    ".seh_context",
    ".seh_endprologue",
    "mov x0, x2",
    "blr x1",
    "str x0, [sp, #{pc}]",
    "mov x0, sp",
    "mov w1, #0",
    "bl {nt_continue}",
    "brk #0xf000",
    ".seh_endproc",
    raise = sym crate::dispatch::raise,
    dispatch = sym crate::dispatch::dispatch_exception,
    unwind_to = sym unwind_to,
    nt_continue = sym crate::NtContinue,
    record = const RECORD_OFFSET,
    size = const CONTEXT_SIZE,
    pc = const CTX_PC,
);
