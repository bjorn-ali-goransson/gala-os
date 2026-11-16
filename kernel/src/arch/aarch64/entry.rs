//! The kernel's entry from Limine and its exception vectors.
//!
//! Limine (base revision 6) enters at EL1, or at EL2 with VHE, with the
//! MMU on, interrupts masked, a 64 KiB stack and FP/SIMD trapped
//! (`CPACR_EL1` = 0). The kernel is soft-float
//! (`aarch64-unknown-none-softfloat`, as x86-64's kernel uses no SSE): it
//! never touches the FP/SIMD registers, so an interrupt need not save
//! them and they are user mode's alone; the entry turns FP/SIMD on for
//! EL0. It calls `kmain` (its arguments zero: not a kernel handover) on
//! the boot stack moved to SP_EL1 (the protocol does not say which SP is
//! in use, and an exception at EL1 runs on SP_EL1: a kernel on SP_EL0
//! would take its first exception on a stale stack). With VHE the `_EL1`
//! names reach the EL2 registers, so the same code serves both.
//!
//! The vectors (`VBAR_EL1`, 16 entries of 128 bytes): the IRQ entries
//! (each origin's second) and the synchronous one from EL0 (an SVC, an
//! abort) save a `TrapFrame` on the stack, call `interrupts::irq` or
//! `interrupts::lower_sync` and return through `gala_aarch64_frame_return`
//! (`eret`), which `syscall::enter_user_regs` also takes; every other
//! exception (a synchronous one at EL1: a kernel bug) is reported and
//! stops the kernel (`trap`). From EL0 the frame is at the top of the
//! thread's kernel stack: SP_EL1 is set there before every return to EL0.

core::arch::global_asm!(
    ".section .text.entry, \"ax\"",
    ".global gala_aarch64_entry",
    "gala_aarch64_entry:",
    // CPACR_EL1.FPEN = 0b11: FP/SIMD not trapped at EL0 or EL1.
    "mov x9, #(3 << 20)",
    "msr cpacr_el1, x9",
    "isb",
    // The stack Limine gave, as SP_EL1: Limine may enter with SPSel 0
    // (on SP_EL0), and an exception taken at EL1 switches to SP_EL1,
    // which would hold whatever the firmware left there.
    "mov x9, sp",
    "msr spsel, #1",
    "mov sp, x9",
    // The GIC's system-register interface (ICC_SRE_EL1.SRE) and its
    // priority mask at "interrupts off" (`cpu::PMR_OFF`): interrupts are
    // masked by priority from here on.
    "mrs x9, S3_0_C12_C12_5",
    "orr x9, x9, #1",
    "msr S3_0_C12_C12_5, x9",
    "isb",
    "mov x9, #{pmr_off}",
    "msr S3_0_C4_C6_0, x9",
    "mov x0, xzr",
    "mov x1, xzr",
    "mov x2, xzr",
    "b kmain",
    "",
    ".section .text.vectors, \"ax\"",
    // A `TrapFrame` (288 bytes) below the current SP_EL1: x0-x30, the
    // interrupted SP (SP_EL0 from EL0, else the one above the frame),
    // ELR, SPSR, ESR, FAR; x0 points at it.
    ".macro GALA_SAVE_FRAME",
    "sub sp, sp, #288",
    "stp x0, x1, [sp, #0]",
    "stp x2, x3, [sp, #16]",
    "stp x4, x5, [sp, #32]",
    "stp x6, x7, [sp, #48]",
    "stp x8, x9, [sp, #64]",
    "stp x10, x11, [sp, #80]",
    "stp x12, x13, [sp, #96]",
    "stp x14, x15, [sp, #112]",
    "stp x16, x17, [sp, #128]",
    "stp x18, x19, [sp, #144]",
    "stp x20, x21, [sp, #160]",
    "stp x22, x23, [sp, #176]",
    "stp x24, x25, [sp, #192]",
    "stp x26, x27, [sp, #208]",
    "stp x28, x29, [sp, #224]",
    "mrs x2, spsr_el1",
    "add x0, sp, #288",
    "tst x2, #0xf",
    "b.ne 1f",
    "mrs x0, sp_el0",
    "1:",
    "stp x30, x0, [sp, #240]",
    "mrs x1, elr_el1",
    "stp x1, x2, [sp, #256]",
    "mrs x3, esr_el1",
    "mrs x4, far_el1",
    "stp x3, x4, [sp, #272]",
    "mov x0, sp",
    ".endm",
    ".balign 2048",
    ".global gala_aarch64_vectors",
    "gala_aarch64_vectors:",
    ".irp n, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15",
    ".balign 128",
    ".if \\n == 8",
    "b gala_aarch64_lower_sync_entry",
    ".elseif (\\n % 4) == 1",
    "b gala_aarch64_irq_entry",
    ".else",
    "mov x0, #\\n",
    "b gala_aarch64_trap_entry",
    ".endif",
    ".endr",
    "",
    "gala_aarch64_irq_entry:",
    "GALA_SAVE_FRAME",
    "bl {irq}",
    "b gala_aarch64_frame_return",
    "",
    // A synchronous exception from EL0 (AArch64): an SVC, an abort, an
    // undefined instruction.
    "gala_aarch64_lower_sync_entry:",
    "GALA_SAVE_FRAME",
    "bl {lower_sync}",
    "b gala_aarch64_frame_return",
    "",
    // Returns with the `TrapFrame` at SP: ELR, SPSR and (to EL0) SP_EL0 as
    // it holds them, the registers, then `eret` (PSTATE.I set since the
    // handler's exit; SPSR's state taken).
    ".global gala_aarch64_frame_return",
    "gala_aarch64_frame_return:",
    "ldp x1, x2, [sp, #256]",
    "msr elr_el1, x1",
    "msr spsr_el1, x2",
    "tst x2, #0xf",
    "b.ne 2f",
    "ldr x0, [sp, #248]",
    "msr sp_el0, x0",
    "2:",
    "ldp x0, x1, [sp, #0]",
    "ldp x2, x3, [sp, #16]",
    "ldp x4, x5, [sp, #32]",
    "ldp x6, x7, [sp, #48]",
    "ldp x8, x9, [sp, #64]",
    "ldp x10, x11, [sp, #80]",
    "ldp x12, x13, [sp, #96]",
    "ldp x14, x15, [sp, #112]",
    "ldp x16, x17, [sp, #128]",
    "ldp x18, x19, [sp, #144]",
    "ldp x20, x21, [sp, #160]",
    "ldp x22, x23, [sp, #176]",
    "ldp x24, x25, [sp, #192]",
    "ldp x26, x27, [sp, #208]",
    "ldp x28, x29, [sp, #224]",
    "ldr x30, [sp, #240]",
    "add sp, sp, #288",
    "eret",
    "",
    "gala_aarch64_trap_entry:",
    "mrs x1, esr_el1",
    "mrs x2, elr_el1",
    "mrs x3, far_el1",
    "mrs x4, spsr_el1",
    "bl {trap}",
    trap = sym trap,
    irq = sym super::interrupts::irq,
    lower_sync = sym super::interrupts::lower_sync,
    pmr_off = const super::cpu::PMR_OFF,
);

const _: () = assert!(super::interrupts::TRAP_FRAME_SIZE == 288);

unsafe extern "C" {
    static gala_aarch64_vectors: u8;
}

/// Installs the exception vectors on this CPU.
pub fn install_vectors() {
    let base = (&raw const gala_aarch64_vectors) as u64;
    // SAFETY: the table above, 2 KiB aligned, holds the 16 entries.
    unsafe {
        core::arch::asm!("msr vbar_el1, {}", "isb", in(reg) base, options(nostack, preserves_flags))
    };
}

/// The 16 vectors in table order: the exception's kind (synchronous, IRQ,
/// FIQ, SError) for each origin (the current EL on SP_EL0, on SP_ELx, a
/// lower EL in AArch64, in AArch32).
const VECTORS: [&str; 16] = [
    "synchronous from EL1 with SP_EL0",
    "IRQ from EL1 with SP_EL0",
    "FIQ from EL1 with SP_EL0",
    "SError from EL1 with SP_EL0",
    "synchronous from EL1 with SP_EL1",
    "IRQ from EL1 with SP_EL1",
    "FIQ from EL1 with SP_EL1",
    "SError from EL1 with SP_EL1",
    "synchronous from EL0 (AArch64)",
    "IRQ from EL0 (AArch64)",
    "FIQ from EL0 (AArch64)",
    "SError from EL0 (AArch64)",
    "synchronous from EL0 (AArch32)",
    "IRQ from EL0 (AArch32)",
    "FIQ from EL0 (AArch32)",
    "SError from EL0 (AArch32)",
];

/// Every exception for now: reported with its syndrome (ESR: the class in
/// bits 26-31), the address it was taken at (ELR), the faulting address
/// (FAR, for aborts) and the saved PSTATE, then the kernel stops (the
/// panic's report and exit).
extern "C" fn trap(vector: u64, esr: u64, elr: u64, far: u64, spsr: u64) -> ! {
    panic!(
        "aarch64: exception: {} (vector {vector}): {} (ESR {esr:#x}, class {:#x}) at {elr:#x}, \
         FAR {far:#x}, SPSR {spsr:#x}: not handled yet",
        VECTORS.get(vector as usize).copied().unwrap_or("?"),
        super::interrupts::exception_name((esr >> 26) as u8),
        esr >> 26
    )
}
