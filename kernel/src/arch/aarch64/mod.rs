//! aarch64: the port under way (ROADMAP AR) on QEMU's `virt` machine.
//! Ported so far: the entry from Limine and the exception vectors
//! (`entry`), the PL011 console through a device page mapped into the
//! bootloader's tables (`serial`, `early_map`), interrupt masking and the
//! counter (`cpu`), the exit through semihosting (`qemu`), the kernel's
//! own page tables (`paging`), the GICv3 (`gic`), the generic timer's
//! tick (`timer`), kernel stacks and the context switch (`context`), the
//! per-CPU block (`percpu`), the other CPUs (`smp`). The kernel boots
//! through its CPUs' start (the `arm_boot` and `arm_sched` scenarios;
//! ARCHITECTURE "The aarch64 port").
//!
//! The rest is still the stub the kernel's aarch64 check (`cargo clippy -p
//! kernel --target aarch64-unknown-none-softfloat`, in CI) builds against: what has
//! an architecture-neutral meaning is filled in (the register frames'
//! accessors over AArch64's registers); everything that would touch the
//! machine ends in `unported`, which never returns.

pub mod backtrace;
pub mod bootphase;
pub mod context;
pub mod cpu;
pub mod debugcon;
pub mod early_map;
pub mod entry;
pub mod fwcfg;
pub mod gic;
pub mod handover;
pub mod hotpatch;
pub mod interrupts;
pub mod paging;
pub mod pci_config;
pub mod percpu;
pub mod port;
pub mod ps2;
pub mod psci;
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

/// The architecture's name in logs ("firmware: UEFI aarch64").
pub const NAME: &str = "aarch64";

/// Whether a process of PE machine `machine` runs here: ARM64 (x64 and
/// x86 programs wait for ROADMAP XT's translator).
pub fn runs_machine(machine: u16) -> bool {
    machine == crate::nt::pe::IMAGE_FILE_MACHINE_ARM64
}

/// The boot CPU's own state: its per-CPU block (TPIDR_EL1) and its
/// exception vectors (`entry`).
pub fn init_boot_cpu() {
    serial::report();
    percpu::init_bsp();
    entry::install_vectors();
}

/// The boot CPU's traps: FP/SIMD is on since the entry and the vectors
/// report every exception (`init_boot_cpu`); system calls and user-mode
/// entry come with the port (ROADMAP AR), and nothing reaches EL0 before.
pub fn init_boot_traps() {}
