//! The processor (stub): interrupt masking (DAIF), the cycle counter
//! (CNTVCT_EL0) and the random generator (RNDR), none of them read yet.

use super::unported;

pub fn halt_forever() -> ! {
    unported("cpu::halt_forever")
}

pub fn interrupts_enabled() -> bool {
    false
}

/// Masks interrupts; whether they were enabled.
pub fn disable_interrupts() -> bool {
    false
}

pub fn enable_interrupts() {
    unported("cpu::enable_interrupts")
}

pub fn enable_interrupts_and_halt() {
    unported("cpu::enable_interrupts_and_halt")
}

/// Called by `IrqMutex` while it spins for a held lock.
pub fn lock_contended() {}

/// The free-running counter the clock reads (CNTVCT_EL0).
pub fn cycles() -> u64 {
    0
}

/// One random word from the processor's generator (RNDR).
pub fn hw_random() -> Option<u64> {
    None
}
