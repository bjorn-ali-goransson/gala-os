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
/// `GalaUnhandledException` passes it: the frames it walked and then
/// seven u32s, the same layout whatever the word size. None when the
/// pointer is null or unreadable: a report without the lines says
/// nothing instead of guessing (`Walk` in user/testntdll and in
/// `nt::crashreport`, which this fills).
pub(in crate::nt) fn walk_of(
    call: &mut super::syscalls::Call<'_>,
    at: u64,
) -> Option<crate::nt::crashreport::Walk> {
    use crate::nt::crashreport::{WALK_FRAMES, Walk};
    if at == 0 {
        return None;
    }
    let mut bytes = [0u8; WALK_FRAMES * 8 + 28];
    call.process.read(at, &mut bytes).ok()?;
    let mut walk = Walk::default();
    for (i, ip) in walk.ips.iter_mut().enumerate() {
        *ip = u64::from_le_bytes(bytes[i * 8..i * 8 + 8].try_into().unwrap_or([0; 8]));
    }
    let word = |i: usize| {
        let at = WALK_FRAMES * 8 + i * 4;
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or([0; 4]))
    };
    walk.frames = word(0);
    walk.handlers = word(1);
    walk.no_entry = word(2);
    walk.kept = word(3).min(WALK_FRAMES as u32);
    walk.no_data = word(4);
    walk.asked = word(5);
    walk.ended = word(6);
    Some(walk)
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
