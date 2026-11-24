//! The processor: interrupt masking, halting (WFI), the free-running
//! counter (CNTVCT_EL0), the random generator (RNDR, `hw_random`).
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

pub(super) fn pmr() -> u64 {
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

/// Puts back a mask `pmr` read: an NMI's exit (`gic::dispatch`), whose
/// handler may have taken an `IrqMutex` -- which writes `PMR_OFF` and,
/// with PSTATE.I set inside the NMI, records interrupts as off and so
/// never writes `PMR_ON` back. The exception return restores PSTATE, not
/// this mask: an NMI that interrupted user mode would otherwise return
/// to it with every interrupt but the NMI masked, for good (the CPU's
/// ticks stopped; `statedump_test`'s check).
pub(super) fn set_pmr(value: u64) {
    // SAFETY: writes ICC_PMR_EL1 (a value read from it), then a barrier.
    unsafe {
        core::arch::asm!("msr S3_0_C4_C6_0, {}", "dsb sy", in(reg) value, options(nomem, nostack, preserves_flags))
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

/// What user mode may do on ARM64, as Windows ARM64 and Linux let it:
/// read the virtual counter (CNTVCT_EL0, and CNTFRQ_EL0 with it:
/// CNTKCTL_EL1.EL0VCTEN; QueryPerformanceCounter and kernel32's pointer
/// encoding read it), read CTR_EL0 (SCTLR_EL1.UCT) and clean and
/// invalidate caches by address to the point of unification
/// (SCTLR_EL1.UCI: `dc cvau`, `ic ivau`, what code that writes code runs
/// before it executes it: FlushInstructionCache, a JIT). The physical
/// counter and the timers stay the kernel's. Once per CPU
/// (`init_boot_cpu`, `smp::ap_main`).
pub fn allow_user_access() {
    const EL0VCTEN: u64 = 1 << 1;
    const UCT: u64 = 1 << 15;
    const UCI: u64 = 1 << 26;
    // SAFETY: CNTKCTL_EL1 only controls EL0's access to the generic
    // timer; SCTLR_EL1's UCT and UCI only let EL0 read the cache type and
    // run cache maintenance by address (which faults like a load where
    // the page is not readable). The other bits keep their values.
    unsafe {
        core::arch::asm!(
            "mrs {t}, cntkctl_el1",
            "orr {t}, {t}, {vct}",
            "msr cntkctl_el1, {t}",
            "mrs {t}, sctlr_el1",
            "orr {t}, {t}, {sct}",
            "msr sctlr_el1, {t}",
            "isb",
            t = out(reg) _,
            vct = in(reg) EL0VCTEN,
            sct = in(reg) UCT | UCI,
            options(nomem, nostack, preserves_flags)
        )
    };
}

/// Makes code written to `[virt, virt + bytes)` (a kernel address: the
/// HHDM's view of a user frame) visible to instruction fetch on every
/// processor, before a user mapping makes it executable: each data cache
/// line cleaned to the point of unification, then each instruction cache
/// line invalidated (by this address: ARMv8's instruction caches behave as
/// physically indexed), with the barriers Arm's sequence for modified
/// code asks for. The line sizes are CTR_EL0's; its IDC and DIC bits say
/// when a step is not needed.
pub fn sync_code(virt: u64, bytes: u64) {
    let ctr: u64;
    // SAFETY: reads CTR_EL0.
    unsafe {
        core::arch::asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack, preserves_flags))
    };
    let end = virt + bytes;
    if ctr & (1 << 28) == 0 {
        let line = 4u64 << ((ctr >> 16) & 0xf);
        let mut at = virt & !(line - 1);
        while at < end {
            // SAFETY: cleans a line of mapped kernel memory (the HHDM).
            unsafe {
                core::arch::asm!("dc cvau, {}", in(reg) at, options(nostack, preserves_flags))
            };
            at += line;
        }
    }
    // SAFETY: a barrier.
    unsafe { core::arch::asm!("dsb ish", options(nostack, preserves_flags)) };
    if ctr & (1 << 29) == 0 {
        let line = 4u64 << (ctr & 0xf);
        let mut at = virt & !(line - 1);
        while at < end {
            // SAFETY: invalidates instruction cache lines; no data changes.
            unsafe {
                core::arch::asm!("ic ivau, {}", in(reg) at, options(nostack, preserves_flags))
            };
            at += line;
        }
    }
    // SAFETY: barriers: the invalidation completes everywhere.
    unsafe { core::arch::asm!("dsb ish", "isb", options(nostack, preserves_flags)) };
}

/// Whether the processor has a random number generator (FEAT_RNG:
/// ID_AA64ISAR0_EL1.RNDR, bits 63:60, nonzero).
fn has_rndr() -> bool {
    id_isar().0 >> 60 != 0
}

/// The instruction set attribute registers ID_AA64ISAR0_EL1 and
/// ID_AA64ISAR1_EL1 (what the processor implements: crypto, CRC32, LSE
/// atomics, dot product, JSCVT, LRCPC, RNDR, ...).
pub fn id_isar() -> (u64, u64) {
    let (isar0, isar1): (u64, u64);
    // SAFETY: reads ID registers (always readable at EL1).
    unsafe {
        core::arch::asm!(
            "mrs {}, id_aa64isar0_el1",
            "mrs {}, id_aa64isar1_el1",
            out(reg) isar0,
            out(reg) isar1,
            options(nomem, nostack, preserves_flags)
        )
    };
    (isar0, isar1)
}

/// One random word from the processor's generator (RNDR, FEAT_RNG), with
/// the retries the architecture allows for (RNDR reports a failure by
/// setting the Z flag); None without FEAT_RNG or if it keeps failing.
pub fn hw_random() -> Option<u64> {
    if !has_rndr() {
        return None;
    }
    for _ in 0..10 {
        let (value, failed): (u64, u64);
        // SAFETY: RNDR exists (FEAT_RNG); it writes the register and the
        // condition flags only.
        unsafe {
            core::arch::asm!(
                "mrs {v}, s3_3_c2_c4_0",
                "cset {f}, eq",
                v = out(reg) value,
                f = out(reg) failed,
                options(nomem, nostack),
            )
        };
        if failed == 0 {
            return Some(value);
        }
    }
    None
}
