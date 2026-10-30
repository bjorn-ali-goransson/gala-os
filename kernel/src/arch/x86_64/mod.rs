//! x86_64 hardware access. This is one of the two places (with `boot`) where
//! `unsafe` is permitted; every `unsafe` block must state why it is sound.

pub mod apic;
pub mod backtrace;
pub mod context;
pub mod cpu;
pub mod debugcon;
pub mod gdt;
pub mod interrupts;
pub mod paging;
pub mod pci;
pub mod percpu;
mod pit;
pub mod port;
pub mod qemu;
pub mod serial;
pub mod smp;
mod sync;
pub mod syscall;
pub mod virtio;
