//! The processor: interrupt masking, halting (WFI), the free-running
//! counter (CNTVCT_EL0). The random generator (RNDR) is not read yet.
//!
//! Interrupts are masked by priority, at the GIC's CPU interface
//! (ICC_PMR_EL1), not by PSTATE.I: "off" lets only the pseudo-NMI's
//! priority through (`PMR_OFF`), "on" every one (`PMR_ON`); PSTATE.I
//! stays clear outside an exception's entry and exit, so the NMI
//! (`gic::NMI_PRIORITY`) reaches a CPU whose interrupts are off (the
//! watchdog's backtrace of a CPU stuck with them off, the profiler's
//! samples). The entries turn the system-register interface on and set
//! `PMR_OFF` first thing (`entry`, `smp`), so these work from the start.

/// ICC_PMR_EL1 with interrupts off: an interrupt is signaled when its
/// priority value is below the mask, so the NMI's (0x80) passes and
/// every other's (`gic::IRQ_PRIORITY`, 0xa0) does not.
pub const PMR_OFF: u64 = 0xa0;
/// ICC_PMR_EL1 with interrupts on: every priority the kernel uses.
pub const PMR_ON: u64 = 0xf0;

fn pmr() -> u64 {
    let v: u64;
    // SAFETY: reads ICC_PMR_EL1 (the system-register interface is on
    // since the entry).
    unsafe {
        core::arch::asm!("mrs {}, S3_0_C4_C6_0", out(reg) v, options(nomem, nostack, preserves_flags))
    };
    v
}

/// Masks by priority (`PMR_OFF`). Writes to ICC_PMR_EL1 are
/// self-synchronizing for masking: nothing below is taken after it.
pub(super) fn pmr_off() {
    // SAFETY: writes ICC_PMR_EL1; no memory is touched.
    unsafe {
        core::arch::asm!("msr S3_0_C4_C6_0, {}", in(reg) PMR_OFF, options(nomem, nostack, preserves_flags))
    };
}

/// Unmasks by priority; the DSB makes a pending interrupt's signal reach
/// the CPU now rather than eventually.
fn pmr_on() {
    // SAFETY: writes ICC_PMR_EL1, then a barrier.
    unsafe {
        core::arch::asm!("msr S3_0_C4_C6_0, {}", "dsb sy", in(reg) PMR_ON, options(nomem, nostack, preserves_flags))
    };
}

/// PSTATE.I clear: exceptions' IRQs (the NMI's while interrupts are off)
/// taken again.
pub(super) fn unmask_daif_irq() {
    // SAFETY: clears PSTATE.I; the vectors are installed.
    unsafe { core::arch::asm!("msr daifclr, #2", options(nomem, nostack, preserves_flags)) };
}

/// PSTATE.I set: nothing is taken (an exception's exit).
pub(super) fn mask_daif_irq() {
    // SAFETY: sets PSTATE.I.
    unsafe { core::arch::asm!("msr daifset, #2", options(nomem, nostack, preserves_flags)) };
}

/// Back to "on" at a normal IRQ's exit, PSTATE.I set until the `eret`
/// (which restores the interrupted code's, clear).
pub(super) fn irq_exit() {
    mask_daif_irq();
    // SAFETY: as pmr_on.
    unsafe {
        core::arch::asm!("msr S3_0_C4_C6_0, {}", in(reg) PMR_ON, options(nomem, nostack, preserves_flags))
    };
}

/// Halts this CPU for good: interrupts off (an NMI is still taken, and
/// the CPU goes back to waiting).
pub fn halt_forever() -> ! {
    disable_interrupts();
    loop {
        // SAFETY: WFI only waits.
        unsafe { core::arch::asm!("wfi", options(nomem, nostack, preserves_flags)) };
    }
}

/// Whether interrupts are on: every priority through and PSTATE.I clear.
pub fn interrupts_enabled() -> bool {
    let daif: u64;
    // SAFETY: reads PSTATE's interrupt masks.
    unsafe {
        core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack, preserves_flags))
    };
    daif & (1 << 7) == 0 && pmr() == PMR_ON
}

/// Masks interrupts (all but the NMI); whether they were on.
pub fn disable_interrupts() -> bool {
    let was = interrupts_enabled();
    pmr_off();
    was
}

/// Unmasks interrupts.
pub fn enable_interrupts() {
    pmr_on();
    unmask_daif_irq();
}

/// Unmasks interrupts and waits for one (WFI wakes on a pending one even
/// while it is masked, so none is missed between the two).
pub fn enable_interrupts_and_halt() {
    enable_interrupts();
    // SAFETY: WFI only waits.
    unsafe { core::arch::asm!("wfi", options(nomem, nostack, preserves_flags)) };
}

/// Called by `IrqMutex` while it spins for a held lock.
pub fn lock_contended() {
    core::hint::spin_loop();
}

/// The free-running counter the clock reads (CNTVCT_EL0).
pub fn cycles() -> u64 {
    let v: u64;
    // SAFETY: reads the virtual counter (always readable at EL1).
    unsafe {
        core::arch::asm!("isb", "mrs {}, cntvct_el0", out(reg) v, options(nomem, nostack, preserves_flags))
    };
    v
}

/// One random word from the processor's generator (RNDR): not read yet
/// (FEAT_RNG is optional; the port checks ID_AA64ISAR0_EL1 first).
pub fn hw_random() -> Option<u64> {
    None
}
