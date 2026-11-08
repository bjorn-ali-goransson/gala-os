//! Linux's ABI on the architecture gala runs on: the part of the Linux
//! personality written in terms of the processor's registers (x86_64:
//! `x86_64`, with the system call table in `lx::syscalls`). The rest of
//! `lx/` uses only this module and `platform`'s architecture-neutral
//! accessors; on aarch64 this module is a stub that compiles.

#[cfg(target_arch = "x86_64")]
mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use self::x86_64::*;

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "aarch64")]
pub use self::aarch64::*;
