//! x86_64 hardware access. This is one of the two places (with `boot`) where
//! `unsafe` is permitted; every `unsafe` block must state why it is sound.

pub mod apic;
pub mod cpu;
pub mod gdt;
pub mod interrupts;
pub mod paging;
mod pit;
pub mod port;
pub mod qemu;
pub mod serial;
mod sync;
