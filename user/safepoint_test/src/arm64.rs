//! safepoint_test's ARM64 parts, as HotSpot's on Windows ARM64: the
//! mutator's loop and the safepoint stub (with their CONTEXT and
//! save-area offsets), the "compiled code" and its constants (in
//! `movz`/`movk` immediates, as HotSpot's AArch64 code embeds object
//! addresses, so a patch rewrites instructions and must be made visible
//! to instruction fetch), NtReleaseSemaphore's stub.
//!
//! The poll is `ldr wzr, [x16]`, x16 loaded from SP_POLL before it (x16,
//! IP0, is the poll's scratch, as r11 on x64: the stub returns through
//! it). The values: x0-x6 and x19-x25 (caller- and callee-saved), 8
//! stack slots, d8-d11. CONTEXT is the public ARM64 one (winnt.h): X[i]
//! at 8 + 8i, Sp 0x100, Pc 0x108, V[i] at 0x110 + 16i.

use core::arch::{asm, global_asm};
use ntrt::{Handle, NtStatus};

use super::{GPRS, VALUES};

/// The registers `sp_mutate` loads, in its order.
const REGS: [usize; GPRS] = [0, 1, 2, 3, 4, 5, 6, 19, 20, 21, 22, 23, 24, 25];
pub const CTX_GPR: [usize; GPRS] = {
    let mut o = [0; GPRS];
    let mut i = 0;
    while i < GPRS {
        o[i] = 8 + 8 * REGS[i];
        i += 1;
    }
    o
};
pub const CTX_SP: usize = 0x100;
pub const CTX_PC: usize = 0x108;
pub const CTX_VEC: usize = 0x110 + 8 * 16;

// sp_mutate(values: x0, out: x1, polls: x2): loads the values into
// registers and stack slots, runs `polls` safepoint polls (with a little
// spinning between them), and stores what the registers and slots hold
// then. Frame: the callee-saved registers it uses, then [out, polls] at
// sp + 64 and the 8 slots at sp.
global_asm!(
    ".globl sp_mutate",
    ".p2align 2",
    "sp_mutate:",
    "stp x29, x30, [sp, #-16]!",
    "stp x19, x20, [sp, #-16]!",
    "stp x21, x22, [sp, #-16]!",
    "stp x23, x24, [sp, #-16]!",
    "stp x25, x26, [sp, #-16]!",
    "stp d8, d9, [sp, #-16]!",
    "stp d10, d11, [sp, #-16]!",
    "stp x1, x2, [sp, #-16]!",
    "sub sp, sp, #64",
    "mov x16, x0",
    "ldr x17, [x16, #112]",
    "str x17, [sp]",
    "ldr x17, [x16, #120]",
    "str x17, [sp, #8]",
    "ldr x17, [x16, #128]",
    "str x17, [sp, #16]",
    "ldr x17, [x16, #136]",
    "str x17, [sp, #24]",
    "ldr x17, [x16, #144]",
    "str x17, [sp, #32]",
    "ldr x17, [x16, #152]",
    "str x17, [sp, #40]",
    "ldr x17, [x16, #160]",
    "str x17, [sp, #48]",
    "ldr x17, [x16, #168]",
    "str x17, [sp, #56]",
    "ldr d8, [x16, #176]",
    "ldr d9, [x16, #184]",
    "ldr d10, [x16, #192]",
    "ldr d11, [x16, #200]",
    "ldp x0, x1, [x16]",
    "ldp x2, x3, [x16, #16]",
    "ldp x4, x5, [x16, #32]",
    "ldr x6, [x16, #48]",
    "ldp x19, x20, [x16, #56]",
    "ldp x21, x22, [x16, #72]",
    "ldp x23, x24, [x16, #88]",
    "ldr x25, [x16, #104]",
    "2:",
    "adrp x16, SP_POLL",
    "ldr x16, [x16, :lo12:SP_POLL]",
    // The poll (0xb940021f): faults while the poll points at the bad page.
    "ldr wzr, [x16]",
    "mov x16, #300",
    "3:",
    "subs x16, x16, #1",
    "b.ne 3b",
    "ldr x17, [sp, #72]",
    "subs x17, x17, #1",
    "str x17, [sp, #72]",
    "b.ne 2b",
    "ldr x16, [sp, #64]",
    "stp x0, x1, [x16]",
    "stp x2, x3, [x16, #16]",
    "stp x4, x5, [x16, #32]",
    "str x6, [x16, #48]",
    "stp x19, x20, [x16, #56]",
    "stp x21, x22, [x16, #72]",
    "stp x23, x24, [x16, #88]",
    "str x25, [x16, #104]",
    "ldr x17, [sp]",
    "str x17, [x16, #112]",
    "ldr x17, [sp, #8]",
    "str x17, [x16, #120]",
    "ldr x17, [sp, #16]",
    "str x17, [x16, #128]",
    "ldr x17, [sp, #24]",
    "str x17, [x16, #136]",
    "ldr x17, [sp, #32]",
    "str x17, [x16, #144]",
    "ldr x17, [sp, #40]",
    "str x17, [x16, #152]",
    "ldr x17, [sp, #48]",
    "str x17, [x16, #160]",
    "ldr x17, [sp, #56]",
    "str x17, [x16, #168]",
    "str d8, [x16, #176]",
    "str d9, [x16, #184]",
    "str d10, [x16, #192]",
    "str d11, [x16, #200]",
    "add sp, sp, #80",
    "ldp d10, d11, [sp], #16",
    "ldp d8, d9, [sp], #16",
    "ldp x25, x26, [sp], #16",
    "ldp x23, x24, [sp], #16",
    "ldp x21, x22, [sp], #16",
    "ldp x19, x20, [sp], #16",
    "ldp x29, x30, [sp], #16",
    "ret",
    // The safepoint stub (as HotSpot's safepoint blob), entered through
    // NtContinue: saves q8-q11 and x0-x30 in a 320-byte area on the
    // stack (q8 at 0, x<n> at 64 + 8n, the resume address at 312, which
    // sp_block fills in), blocks in sp_block(area), restores them and
    // branches back to the instruction after the poll through x16.
    ".globl sp_stub",
    ".p2align 2",
    "sp_stub:",
    "sub sp, sp, #320",
    "stp q8, q9, [sp]",
    "stp q10, q11, [sp, #32]",
    "stp x0, x1, [sp, #64]",
    "stp x2, x3, [sp, #80]",
    "stp x4, x5, [sp, #96]",
    "stp x6, x7, [sp, #112]",
    "stp x8, x9, [sp, #128]",
    "stp x10, x11, [sp, #144]",
    "stp x12, x13, [sp, #160]",
    "stp x14, x15, [sp, #176]",
    "stp x16, x17, [sp, #192]",
    "stp x18, x19, [sp, #208]",
    "stp x20, x21, [sp, #224]",
    "stp x22, x23, [sp, #240]",
    "stp x24, x25, [sp, #256]",
    "stp x26, x27, [sp, #272]",
    "stp x28, x29, [sp, #288]",
    "str x30, [sp, #304]",
    "mov x0, sp",
    "bl {block}",
    "ldp q8, q9, [sp]",
    "ldp q10, q11, [sp, #32]",
    "ldp x0, x1, [sp, #64]",
    "ldp x2, x3, [sp, #80]",
    "ldp x4, x5, [sp, #96]",
    "ldp x6, x7, [sp, #112]",
    "ldp x8, x9, [sp, #128]",
    "ldp x10, x11, [sp, #144]",
    "ldp x12, x13, [sp, #160]",
    "ldp x14, x15, [sp, #176]",
    "ldr x17, [sp, #200]",
    "ldp x18, x19, [sp, #208]",
    "ldp x20, x21, [sp, #224]",
    "ldp x22, x23, [sp, #240]",
    "ldp x24, x25, [sp, #256]",
    "ldp x26, x27, [sp, #272]",
    "ldp x28, x29, [sp, #288]",
    "ldr x30, [sp, #304]",
    "ldr x16, [sp, #312]",
    "add sp, sp, #320",
    "br x16",
    // NtReleaseSemaphore (0x0a, as x64's).
    ".globl sp_release_semaphore",
    ".p2align 2",
    "sp_release_semaphore:",
    "svc #0x0a",
    "ret",
    block = sym super::sp_block,
);

unsafe extern "C" {
    pub fn sp_mutate(values: *const [u64; VALUES], out: *mut [u64; VALUES], polls: u64);
    pub fn sp_release_semaphore(handle: Handle, count: i32, previous: *mut i32) -> NtStatus;
    pub fn sp_stub();
}

/// Offsets in sp_stub's save area of the registers `sp_mutate` loads (in
/// its order), of q8 and of the resume address.
pub const STUB_GPR: [usize; GPRS] = {
    let mut o = [0; GPRS];
    let mut i = 0;
    while i < GPRS {
        o[i] = 64 + 8 * REGS[i];
        i += 1;
    }
    o
};
pub const STUB_VEC: usize = 0;
pub const STUB_RETURN: usize = 312;
/// The mutator's stack pointer at the poll, from the save area.
pub const STUB_SP: usize = 320;

/// The poll's bytes (`ldr wzr, [x16]`).
pub const POLL: &[u8] = &0xb940_021fu32.to_le_bytes();

/// `movz x0/w0, #imm16, lsl #(16 * hw)` and `movk` (opc 0b10, 0b11);
/// `sf` 1 for the 64-bit forms.
const fn mov_wide(sf: u32, keep: bool, hw: u32, imm: u32, rd: u32) -> u32 {
    let opc = if keep { 0b11 } else { 0b10 };
    sf << 31 | opc << 29 | 0b100101 << 23 | hw << 21 | (imm & 0xffff) << 5 | rd
}

/// The imm16 of a move-wide instruction.
fn imm16(insn: u32) -> u32 {
    (insn >> 5) & 0xffff
}

const RET: u32 = 0xd65f_03c0;

/// Writes mutator compiled code returning `value` at `at`: `movz x0,
/// #v0; movk x0, #v1, lsl 16; movk x0, #v2, lsl 32; movk x0, #v3, lsl 48;
/// ret` (HotSpot's AArch64 form of an embedded object address).
///
/// # Safety
/// `at` is 20 writable, 4-aligned bytes of the code page; no thread runs
/// it.
pub unsafe fn write_code(at: usize, value: u64) {
    let p = at as *mut u32;
    // SAFETY: the caller's.
    unsafe {
        for hw in 0..4 {
            p.add(hw).write(mov_wide(1, hw != 0, hw as u32, (value >> (16 * hw)) as u32, 0));
        }
        p.add(4).write(RET);
    }
}

/// The constant the compiled code at `at` returns.
///
/// # Safety
/// `at` holds `write_code`'s code.
pub unsafe fn read_code(at: usize) -> u64 {
    let p = at as *const u32;
    // SAFETY: the caller's.
    (0..4).fold(0, |v, hw| v | u64::from(imm16(unsafe { p.add(hw).read() })) << (16 * hw))
}

/// The loopers' code, called as f(out: x0, iterations: x1, unused: x2,
/// poll pointer: x3): `loop: ldr x16, [x3]; ldr wzr, [x16] (the poll);
/// movz w8, #lo; movk w8, #hi, lsl 16; str w8, [x0]; subs x1, x1, #1;
/// b.ne loop; ret`. After each stop at the poll it stores the constant
/// again, so `*out` must be the newest.
const LOOP_INSNS: [u32; 8] = [
    0xf940_0070,                 // ldr x16, [x3]
    0xb940_021f,                 // ldr wzr, [x16]
    mov_wide(0, false, 0, 0, 8), // movz w8, #0
    mov_wide(0, true, 1, 0, 8),  // movk w8, #0, lsl 16
    0xb900_0008,                 // str w8, [x0]
    0xf100_0421,                 // subs x1, x1, #1
    0x54ff_ff41,                 // b.ne loop (-24)
    RET,
];
pub const LOOP_CODE: &[u8] = &{
    let mut b = [0u8; 32];
    let mut i = 0;
    while i < 8 {
        let w = LOOP_INSNS[i].to_le_bytes();
        b[4 * i] = w[0];
        b[4 * i + 1] = w[1];
        b[4 * i + 2] = w[2];
        b[4 * i + 3] = w[3];
        i += 1;
    }
    b
};
/// Offset of LOOP_CODE in the code page (instructions are 4-aligned).
pub const LOOP_CODE_AT: usize = 0x100;
/// Its movz and movk.
const LOOP_MOVZ: usize = 8;

/// The 32-bit constant LOOP_CODE at `at` stores.
///
/// # Safety
/// `at` holds LOOP_CODE.
pub unsafe fn read_loop(at: usize) -> u32 {
    let p = (at + LOOP_MOVZ) as *const u32;
    // SAFETY: the caller's.
    unsafe { imm16(p.read()) | imm16(p.add(1).read()) << 16 }
}

/// Makes LOOP_CODE at `at` store `value`.
///
/// # Safety
/// `at` holds LOOP_CODE; no thread runs it.
pub unsafe fn write_loop(at: usize, value: u32) {
    let p = (at + LOOP_MOVZ) as *mut u32;
    // SAFETY: the caller's.
    unsafe {
        p.write(mov_wide(0, false, 0, value, 8));
        p.add(1).write(mov_wide(0, true, 1, value >> 16, 8));
    }
}

/// Makes code written at [base, base + size) visible to instruction
/// fetch, as FlushInstructionCache does on ARM64 (kernel32's
/// `arch::sync_code`; the test ships no kernel32): each data cache line
/// cleaned to the point of unification, then each instruction cache line
/// invalidated, with the barriers between (the Arm ARM's sequence for
/// self-modifying code; the line sizes from CTR_EL0).
pub fn sync_code(base: usize, size: usize) {
    let ctr: u64;
    // SAFETY: reading CTR_EL0, which EL0 may (SCTLR_EL1.UCT).
    unsafe { asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack)) };
    let dline = 4usize << ((ctr >> 16) & 0xf);
    let iline = 4usize << (ctr & 0xf);
    let end = base + size;
    let mut at = base & !(dline - 1);
    while at < end {
        // SAFETY: cache maintenance by address of our own mapped code page.
        unsafe { asm!("dc cvau, {}", in(reg) at, options(nostack)) };
        at += dline;
    }
    // SAFETY: a barrier.
    unsafe { asm!("dsb ish", options(nostack)) };
    let mut at = base & !(iline - 1);
    while at < end {
        // SAFETY: as above.
        unsafe { asm!("ic ivau, {}", in(reg) at, options(nostack)) };
        at += iline;
    }
    // SAFETY: barriers.
    unsafe { asm!("dsb ish", "isb", options(nostack)) };
}
