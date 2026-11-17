//! Windows' ABI on the architecture gala runs on: the part of the NT layer
//! that is written in terms of the processor's registers and Windows'
//! records for them (x64: `x64`, the CONTEXT record, where a system call's
//! arguments are, how threads start and exceptions and APCs are
//! delivered). The rest of the NT layer uses only what this module and
//! `platform`'s architecture-neutral accessors (`UserRegs::ip`,
//! `SyscallFrame::number`, ...) give it; on another architecture this
//! module is its own (aarch64: a stub that compiles, so that the kernel's
//! aarch64 check finds x64 assumptions elsewhere in `nt/`).

/// gala's ntdll's record of how the search for a handler went, as
/// `GalaUnhandledException` passes it: three u32s (frames unwound,
/// handlers asked, frames no function entry was found for), whatever the
/// word size. None when the pointer is null or unreadable: a report
/// without the line says nothing instead of guessing (`Walk` in
/// user/testntdll, `nt::crashreport`).
pub(in crate::nt) fn walk_of(
    call: &mut super::syscalls::Call<'_>,
    at: u64,
) -> Option<(u32, u32, u32)> {
    if at == 0 {
        return None;
    }
    let mut bytes = [0u8; 12];
    call.process.read(at, &mut bytes).ok()?;
    let word = |i: usize| u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap_or([0; 4]));
    Some((word(0), word(1), word(2)))
}

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

// ARM64's CONTEXT layout is plain data: the host's tests build it whatever
// the host is.
#[cfg(all(test, not(target_arch = "aarch64")))]
#[path = "arm64/context.rs"]
mod arm64_context;
