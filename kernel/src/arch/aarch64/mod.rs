//! aarch64: a stub of the architecture interface that compiles and runs
//! nothing (ROADMAP PT; the ARM64 port is in "Later"). Its purpose is the
//! kernel's aarch64 check (`cargo clippy -p kernel --target
//! aarch64-unknown-none`, in CI): code outside `kernel/src/arch` that
//! names an x86-64 type, register, instruction or `core::arch::x86_64`
//! does not build here.
//!
//! What has an architecture-neutral meaning is filled in where that costs
//! nothing (the register frames' accessors over AArch64's registers, the
//! interrupt-state helpers); everything that would touch the machine ends
//! in `unported`, which never returns. No boot is expected.

pub mod backtrace;
pub mod bootphase;
pub mod context;
pub mod cpu;
pub mod debugcon;
pub mod handover;
pub mod interrupts;
pub mod paging;
pub mod pci_config;
pub mod percpu;
pub mod port;
pub mod ps2;
pub mod qemu;
pub mod serial;
pub mod smp;
pub mod syscall;
pub mod timer;
pub mod xstate;

/// Where the port has not been written yet: stops here.
#[track_caller]
fn unported(what: &str) -> ! {
    panic!("aarch64: {what} is not ported yet")
}

/// The boot CPU's own state (exception level, per-CPU block).
pub fn init_boot_cpu() {
    unported("init_boot_cpu")
}

/// The boot CPU's exception vectors, system calls and FP/SIMD state.
pub fn init_boot_traps() {
    unported("init_boot_traps")
}
