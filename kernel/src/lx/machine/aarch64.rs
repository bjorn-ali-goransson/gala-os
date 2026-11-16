//! Linux's AArch64 ABI: arguments in x0-x5, the number in x8, the result
//! in x0 (`svc #0`); a program starts with sp at argc; a restarted call
//! goes back to its `svc` with x0 its first argument again (the result
//! overwrote it); AT_PLATFORM and AT_HWCAP (musl's
//! `arch/aarch64/bits/hwcap.h`, MIT, read as data); the signal frame:
//! AArch64's rt_sigframe (siginfo, then the ucontext whose sigcontext
//! keeps x0-x30, sp, pc and pstate, and in its reserved area an
//! fpsimd_context record with FPSR, FPCR and V0-V31; musl's
//! `arch/aarch64/bits/signal.h`), then a frame record (x29, x30) the
//! handler's x29 points at.

use lx_abi::signal::{self as abi, KSigaction, SigSet, Siginfo, Stack};

use crate::nt::platform::{Gpr, SyscallFrame, UserRegs, XState};
use crate::nt::process::Process;

/// System call argument `i` (0-based, below 6).
pub fn syscall_arg(frame: &SyscallFrame, i: usize) -> u64 {
    [Gpr::X0, Gpr::X1, Gpr::X2, Gpr::X3, Gpr::X4, Gpr::X5].map(|r| frame.gpr(r))[i]
}

/// clone(2)'s argument positions for (flags, stack, parent_tid,
/// child_tid, tls): aarch64 passes tls before child_tid (the kernel's
/// CLONE_BACKWARDS order, which musl's `arch/aarch64` __clone follows).
pub const CLONE_ARGS: [usize; 5] = [0, 1, 2, 4, 3];

/// The `svc` instruction's length: a restarted call goes back to it.
const SVC_LEN: u64 = 4;

/// Makes the interrupted call `number` again when it returns to `regs`:
/// the pc back on the `svc`, the number in x8 and the first argument back
/// in x0 (where the call's result went).
pub fn restart(regs: &mut UserRegs, number: u64, arg0: u64) {
    regs.set_gpr(Gpr::X8, number);
    regs.set_gpr(Gpr::X0, arg0);
    regs.set_ip(regs.ip() - SVC_LEN);
}

/// The registers a program starts with: the ELF entry with sp at argc and
/// every other register zero (x0 0: no function for atexit).
pub fn start_regs(entry: u64, sp: u64) -> UserRegs {
    let mut regs = UserRegs::default();
    regs.set_sp(sp);
    regs.set_ip(entry);
    regs
}

/// An exec'd program's floating-point control: its thread starts with
/// the reset FP/SIMD state (FPCR 0), which exec keeps.
pub fn reset_fp_control() {}

/// The open flags whose values differ between architectures: aarch64's
/// (musl's `arch/aarch64/bits/fcntl.h`; O_DIRECT, 0o200000, is not
/// taken).
pub const O_LARGEFILE: u64 = 0o400000;
pub const O_DIRECTORY: u64 = 0o40000;
pub const O_NOFOLLOW: u64 = 0o100000;

/// `struct stat` in the generic layout aarch64 uses (128 bytes).
pub fn stat_bytes(st: &lx_abi::layout::Stat) -> [u8; 128] {
    st.to_generic_bytes()
}

/// seccomp_data's and SIGSYS's architecture: AUDIT_ARCH_AARCH64 (EM_AARCH64 with the 64-bit and little-endian bits).
pub const AUDIT_ARCH: u32 = 0xc000_00b7;

/// struct epoll_event's data offset: naturally aligned on aarch64.
pub const EPOLL_DATA_AT: usize = 8;

/// AT_PLATFORM's string.
pub const PLATFORM: &[u8] = b"aarch64";

const HWCAP_FP: u64 = 1 << 0;
const HWCAP_ASIMD: u64 = 1 << 1;
const HWCAP_AES: u64 = 1 << 3;
const HWCAP_PMULL: u64 = 1 << 4;
const HWCAP_SHA1: u64 = 1 << 5;
const HWCAP_SHA2: u64 = 1 << 6;
const HWCAP_CRC32: u64 = 1 << 7;
const HWCAP_ATOMICS: u64 = 1 << 8;

/// AT_HWCAP: FP and ASIMD (every ARMv8-A processor gala runs on has them)
/// and what ID_AA64ISAR0_EL1 says (AES at 4, PMULL its value 2; SHA1 at
/// 8; SHA2 at 12; CRC32 at 16; LSE atomics at 20, value 2).
pub fn hwcap() -> u64 {
    hwcap_of(crate::nt::platform::id_isar().0)
}

fn hwcap_of(isar0: u64) -> u64 {
    let field = |at: u32| (isar0 >> at) & 0xf;
    let mut caps = HWCAP_FP | HWCAP_ASIMD;
    for (bit, at, min) in [
        (HWCAP_AES, 4, 1),
        (HWCAP_PMULL, 4, 2),
        (HWCAP_SHA1, 8, 1),
        (HWCAP_SHA2, 12, 1),
        (HWCAP_CRC32, 16, 1),
        (HWCAP_ATOMICS, 20, 2),
    ] {
        if field(at) >= min {
            caps |= bit;
        }
    }
    caps
}

// The frame's layout (offsets from its base, the handler's sp).
const INFO: usize = 0;
const UC: usize = 128;
const UC_FLAGS: usize = UC;
const UC_STACK: usize = UC + 16;
const UC_SIGMASK: usize = UC + 40;
/// uc_mcontext (struct sigcontext), 16-aligned after uc_sigmask's 128
/// bytes.
const MC: usize = UC + 176;
const MC_REGS: usize = MC + 8;
const MC_SP: usize = MC + 256;
const MC_PC: usize = MC + 264;
const MC_PSTATE: usize = MC + 272;
/// sigcontext's __reserved[4096], 16-aligned: the records.
const MC_RESERVED: usize = MC + 288;
const RESERVED_SIZE: usize = 4096;
/// The frame record (x29, x30) after the rt_sigframe.
const RECORD: usize = MC_RESERVED + RESERVED_SIZE;
pub const FRAME_SIZE: usize = RECORD + 16;
/// fpsimd_context: its head (magic, size), FPSR, FPCR, then V0-V31.
const FPSIMD_MAGIC: u32 = 0x4650_8001;
const FPSIMD_SIZE: usize = 528;

/// Where rt_sigreturn finds the frame's uc_stack: sp is the frame's base.
pub const UC_STACK_AT: u64 = UC_STACK as u64;

const X: [Gpr; 31] = [
    Gpr::X0,
    Gpr::X1,
    Gpr::X2,
    Gpr::X3,
    Gpr::X4,
    Gpr::X5,
    Gpr::X6,
    Gpr::X7,
    Gpr::X8,
    Gpr::X9,
    Gpr::X10,
    Gpr::X11,
    Gpr::X12,
    Gpr::X13,
    Gpr::X14,
    Gpr::X15,
    Gpr::X16,
    Gpr::X17,
    Gpr::X18,
    Gpr::X19,
    Gpr::X20,
    Gpr::X21,
    Gpr::X22,
    Gpr::X23,
    Gpr::X24,
    Gpr::X25,
    Gpr::X26,
    Gpr::X27,
    Gpr::X28,
    Gpr::X29,
    Gpr::X30,
];

fn u64_at(b: &[u8], at: usize) -> u64 {
    b.get(at..at + 8).and_then(|s| s.try_into().ok()).map_or(0, u64::from_le_bytes)
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).and_then(|s| s.try_into().ok()).map_or(0, u32::from_le_bytes)
}

/// What rt_sigreturn finds at the stack pointer `sp` (the frame's base:
/// the handler returned through x30 with sp as it got it): the registers,
/// the floating-point state (its fpsimd_context; None without one) and
/// the signal mask the frame saved. Only the condition flags of pstate
/// reach user mode (the return to EL0 filters it).
pub fn read_sigframe(
    process: &Process,
    sp: u64,
) -> Result<(UserRegs, Option<XState>, SigSet), &'static str> {
    let mut b = alloc::vec::Vec::new();
    b.try_reserve_exact(FRAME_SIZE).map_err(|_| "no memory to read the frame")?;
    b.resize(FRAME_SIZE, 0);
    process.read(sp, &mut b).map_err(|_| "the frame is not readable")?;
    let mut regs = UserRegs::default();
    for (i, r) in X.iter().enumerate() {
        regs.set_gpr(*r, u64_at(&b, MC_REGS + 8 * i));
    }
    regs.set_sp(u64_at(&b, MC_SP));
    regs.set_ip(u64_at(&b, MC_PC));
    regs.set_flags(u64_at(&b, MC_PSTATE));
    let fp = (u32_at(&b, MC_RESERVED) == FPSIMD_MAGIC
        && u32_at(&b, MC_RESERVED + 4) as usize == FPSIMD_SIZE)
        .then(|| {
            let mut fx = XState::initial();
            let x = fx.bytes_mut();
            x[..512].copy_from_slice(&b[MC_RESERVED + 16..MC_RESERVED + 16 + 512]);
            // XState: FPCR then FPSR after V0-V31; the record: FPSR, FPCR.
            x[512..516].copy_from_slice(&b[MC_RESERVED + 12..MC_RESERVED + 16]);
            x[520..524].copy_from_slice(&b[MC_RESERVED + 8..MC_RESERVED + 12]);
            fx
        });
    Ok((regs, fp, SigSet(u64_at(&b, UC_SIGMASK))))
}

/// A handler's frame, ready to be written at `addr` (its floating-point
/// state inside it: `fp_len` 0), and the registers the handler starts
/// with.
pub struct Frame {
    pub addr: u64,
    pub bytes: alloc::boxed::Box<[u8; FRAME_SIZE]>,
    pub fpstate: u64,
    pub fp_len: usize,
    pub regs: UserRegs,
}

/// Builds the rt_sigframe for `sig` (`info`, handled by `action`) that
/// interrupts `regs`, whose mask was `old_mask` and floating-point state
/// `fx`. None if the stack pointer leaves no room (or no memory).
///
/// Below `sp` (the interrupted sp, or the top of the alternate stack),
/// 16-aligned: the frame. The handler starts with pc = sa_handler, sp =
/// the frame, x0 = sig, x29 = the frame record (the interrupted x29 and
/// x30), x30 = sa_restorer (where it returns), and with SA_SIGINFO x1 =
/// &siginfo and x2 = &ucontext; every other register as it was.
#[allow(clippy::too_many_arguments)]
pub fn build_frame(
    regs: &UserRegs,
    sp: u64,
    uc_stack: Stack,
    sig: u32,
    info: &Siginfo,
    action: &KSigaction,
    old_mask: SigSet,
    fx: &XState,
) -> Option<Frame> {
    let addr = sp.checked_sub(FRAME_SIZE as u64)? & !15;
    let mut bytes = crate::mm::heap::try_box([0u8; FRAME_SIZE]).ok()?;
    let b = &mut *bytes;
    let mut put = |at: usize, v: &[u8]| b[at..at + v.len()].copy_from_slice(v);
    put(INFO, &info.to_bytes());
    put(UC_FLAGS, &0u64.to_le_bytes());
    put(UC_STACK, &uc_stack.ss_sp.to_le_bytes());
    put(UC_STACK + 8, &(uc_stack.ss_flags as u32).to_le_bytes());
    put(UC_STACK + 16, &uc_stack.ss_size.to_le_bytes());
    put(UC_SIGMASK, &old_mask.0.to_le_bytes());
    for (i, r) in X.iter().enumerate() {
        put(MC_REGS + 8 * i, &regs.gpr(*r).to_le_bytes());
    }
    put(MC_SP, &regs.sp().to_le_bytes());
    put(MC_PC, &regs.ip().to_le_bytes());
    put(MC_PSTATE, &regs.flags().to_le_bytes());
    let x = fx.bytes();
    put(MC_RESERVED, &FPSIMD_MAGIC.to_le_bytes());
    put(MC_RESERVED + 4, &(FPSIMD_SIZE as u32).to_le_bytes());
    // FPSR and FPCR (32 bits each), from XState's FPSR at 520, FPCR at 512.
    put(MC_RESERVED + 8, &x[520..524]);
    put(MC_RESERVED + 12, &x[512..516]);
    put(MC_RESERVED + 16, &x[..512]);
    // The records end with a zero head (magic 0, size 0): already zero.
    put(RECORD, &regs.gpr(Gpr::X29).to_le_bytes());
    put(RECORD + 8, &regs.gpr(Gpr::X30).to_le_bytes());
    let mut handler = *regs;
    handler.set_ip(action.handler);
    handler.set_sp(addr);
    handler.set_gpr(Gpr::X0, u64::from(sig));
    if action.flags & abi::SA_SIGINFO != 0 {
        handler.set_gpr(Gpr::X1, addr + INFO as u64);
        handler.set_gpr(Gpr::X2, addr + UC as u64);
    }
    handler.set_gpr(Gpr::X29, addr + RECORD as u64);
    handler.set_gpr(Gpr::X30, action.restorer);
    Some(Frame { addr, bytes, fpstate: 0, fp_len: 0, regs: handler })
}
