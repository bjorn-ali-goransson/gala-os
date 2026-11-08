//! Windows' ABI on the architecture gala runs on: the part of the NT layer
//! that is written in terms of the processor's registers and Windows'
//! records for them (x64: `x64`, the CONTEXT record, where a system call's
//! arguments are, how threads start and exceptions and APCs are
//! delivered). The rest of the NT layer uses only what this module and
//! `platform`'s architecture-neutral accessors (`UserRegs::ip`,
//! `SyscallFrame::number`, ...) give it; on another architecture this
//! module is its own (aarch64: a stub that compiles, so that the kernel's
//! aarch64 check finds x64 assumptions elsewhere in `nt/`).

/// Where a system call argument is: in a register (its value) or on the
/// user stack (the address to read it from).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgSlot {
    Value(u64),
    Stack(u64),
}

#[cfg(target_arch = "x86_64")]
mod x64;
#[cfg(target_arch = "x86_64")]
pub use x64::*;

#[cfg(target_arch = "aarch64")]
mod arm64;
#[cfg(target_arch = "aarch64")]
pub use arm64::*;
