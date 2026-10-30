//! CPU control.

use core::arch::asm;

/// Disables interrupts and halts this CPU forever.
pub fn halt_forever() -> ! {
    loop {
        // SAFETY: `cli; hlt` only stops this CPU; it touches no memory.
        unsafe { asm!("cli", "hlt", options(nomem, nostack)) };
    }
}
