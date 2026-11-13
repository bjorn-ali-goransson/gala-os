//! x86_64 hardware access. This is one of the two places (with `boot`) where
//! `unsafe` is permitted; every `unsafe` block must state why it is sound.

mod apboot;
pub mod apic;
pub mod backtrace;
pub mod bootphase;
pub mod context;
pub mod cpu;
pub mod debugcon;
pub mod fwcfg;
pub mod gdt;
pub mod handover;
pub mod interrupts;
pub mod paging;
pub mod pci_config;
pub mod percpu;
mod pit;
pub mod port;
pub mod ps2;
pub mod qemu;
pub mod refclock;
pub mod serial;
pub mod smp;
mod sync;
pub mod syscall;
pub mod timer;
pub mod xstate;

/// The boot CPU's own state, first thing (before lock-order checking,
/// which needs the CPU's index): x86-64's GDT and TSS, the per-CPU block.
pub fn init_boot_cpu() {
    gdt::init_bsp();
    percpu::init_bsp();
}

/// The boot CPU's traps and system calls, and the user's floating-point
/// state (boot phases `xsave`, `gdt_idt`): x86-64's SSE/XSAVE, IDT and
/// SYSCALL.
pub fn init_boot_traps() {
    cpu::enable_sse();
    bootphase::mark("xsave");
    interrupts::init();
    syscall::init_cpu();
    bootphase::mark("gdt_idt");
    let vendor = cpu::vendor();
    crate::info!(
        "cpu: GDT, TSS and IDT loaded ({})",
        core::str::from_utf8(&vendor).unwrap_or("unknown vendor")
    );
}
