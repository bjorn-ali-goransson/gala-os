//! Linux's AArch64 ABI (stub): arguments in x0-x5, the number in x8, the
//! result in x0; the signal frame (AArch64's rt_sigframe with its
//! fpsimd_context) is not written yet.

use lx_abi::signal::{KSigaction, SigSet, Siginfo};

use crate::nt::platform::{Gpr, SyscallFrame, UserRegs};
use crate::nt::process::Process;

#[track_caller]
fn unported(what: &str) -> ! {
    panic!("lx on aarch64: {what} is not ported yet")
}

pub fn syscall_arg(frame: &SyscallFrame, i: usize) -> u64 {
    [Gpr::X0, Gpr::X1, Gpr::X2, Gpr::X3, Gpr::X4, Gpr::X5].map(|r| frame.gpr(r))[i]
}

pub fn restart(_regs: &mut UserRegs, _number: u64) {
    unported("restarting a system call")
}

/// The ELF entry with sp at argc, every other register zero.
pub fn start_regs(entry: u64, sp: u64) -> UserRegs {
    let mut regs = UserRegs::default();
    regs.set_sp(sp);
    regs.set_ip(entry);
    regs
}

pub fn reset_fp_control() {}

/// AT_PLATFORM's string.
pub const PLATFORM: &[u8] = b"aarch64";

/// AT_HWCAP (ID_AA64*'s features, not read yet).
pub fn hwcap() -> u64 {
    0
}

pub fn read_sigframe(
    _process: &Process,
    _sp: u64,
) -> Result<(UserRegs, u64, SigSet), &'static str> {
    Err("the AArch64 signal frame is not ported yet")
}

/// A handler's frame, ready to be written at `addr`.
pub struct Frame {
    pub addr: u64,
    pub bytes: [u8; 0],
    pub fpstate: u64,
    pub regs: UserRegs,
}

pub fn build_frame(
    _regs: &UserRegs,
    _sig: u32,
    _info: &Siginfo,
    _action: &KSigaction,
    _old_mask: SigSet,
    _fp_len: u64,
) -> Option<Frame> {
    None
}
