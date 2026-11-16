//! Ending a QEMU run with a result: Arm semihosting's SYS_EXIT_EXTENDED
//! (QEMU with `-semihosting-config enable=on`, which xtask passes on
//! aarch64), with the exit statuses x86-64's isa-debug-exit gives:
//! `(code << 1) | 1`, 33 for success and 35 for failure, so the harness
//! reads one meaning on both.

use super::cpu;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    /// QEMU exit status 33.
    Success = 0x10,
    /// QEMU exit status 35.
    Failure = 0x11,
}

/// Semihosting operation SYS_EXIT_EXTENDED and its reason
/// ADP_Stopped_ApplicationExit (Arm's semihosting specification).
const SYS_EXIT_EXTENDED: u64 = 0x20;
const ADP_STOPPED_APPLICATION_EXIT: u64 = 0x2_0026;

/// Terminates QEMU. Only called when the kernel command line opts in
/// (`qemu_exit`): without a semihosting host the HLT traps (and the
/// exception reports it), so other machines never get here.
pub fn exit(code: QemuExitCode) -> ! {
    let block = [ADP_STOPPED_APPLICATION_EXIT, u64::from(code as u32) << 1 | 1];
    let result: u64;
    // SAFETY: the semihosting call reads the two words; QEMU ends.
    unsafe {
        core::arch::asm!(
            "hlt #0xf000",
            inout("x0") SYS_EXIT_EXTENDED => result,
            in("x1") block.as_ptr(),
            options(nostack, readonly, preserves_flags)
        )
    };
    // Only when the host did not end the run (it could not read the
    // block, or there is no semihosting host): said, rather than a silent
    // halt the harness can only time out on.
    crate::console::emergency_line(format_args!(
        "qemu: the run did not end: the semihosting exit returned {result:#x} (its block at \
         {:#x}); halting",
        block.as_ptr() as u64
    ));
    cpu::halt_forever()
}
