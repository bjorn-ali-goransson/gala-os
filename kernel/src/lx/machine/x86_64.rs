//! Linux's x86-64 ABI: what the Linux personality does in terms of
//! x86-64's registers. Where a system call's arguments are (rdi, rsi,
//! rdx, r10, r8, r9; the number in rax, the result in rax), how a program
//! starts (rsp at argc), how a restarted call goes back to its `syscall`
//! instruction, AT_PLATFORM and AT_HWCAP, and the signal frame
//! (`lx_abi::signal`'s rt_sigframe and sigcontext, the red zone, the
//! handler's entry registers).

use lx_abi::signal::{
    self as abi, KSigaction, RtSigframe, SigSet, Sigcontext, Siginfo, Stack, Ucontext,
};

use crate::nt::platform::{self, Gpr::*, SyscallFrame, UserRegs, XState, xstate};
use crate::nt::process::Process;

/// System call argument `i` (0-based, below 6).
pub fn syscall_arg(frame: &SyscallFrame, i: usize) -> u64 {
    [frame.gpr(Rdi), frame.gpr(Rsi), frame.gpr(Rdx), frame.gpr(R10), frame.gpr(R8), frame.gpr(R9)]
        [i]
}

/// clone(2)'s argument positions for (flags, stack, parent_tid,
/// child_tid, tls): x86_64's order.
pub const CLONE_ARGS: [usize; 5] = [0, 1, 2, 3, 4];

/// The `syscall` instruction's length: a restarted call goes back to it.
const SYSCALL_LEN: u64 = 2;

/// Makes the interrupted call `number` again when it returns to `regs`:
/// the number back in rax, rip back on the `syscall` instruction (the
/// arguments are where they were: `_arg0` is aarch64's need).
pub fn restart(regs: &mut UserRegs, number: u64, _arg0: u64) {
    regs.set_gpr(Rax, number);
    regs.set_ip(regs.ip() - SYSCALL_LEN);
}

/// The registers a program starts with: the ELF entry with rsp at argc
/// and every other register zero (rdx 0: no function for atexit; SysV
/// ABI, "Process initialization").
pub fn start_regs(entry: u64, sp: u64) -> UserRegs {
    let mut regs = UserRegs::default();
    regs.set_sp(sp);
    regs.set_ip(entry);
    regs
}

/// An exec'd program's floating-point control: SSE's default MXCSR.
pub fn reset_fp_control() {
    platform::set_mxcsr(0x1f80);
}

/// The open flags whose values differ between architectures (x86_64's,
/// asm-generic/fcntl.h's defaults).
pub const O_LARGEFILE: u64 = 0o100000;
pub const O_DIRECTORY: u64 = 0o200000;
pub const O_NOFOLLOW: u64 = 0o400000;

/// `struct stat` as x86_64 lays it out (144 bytes).
pub fn stat_bytes(st: &lx_abi::layout::Stat) -> [u8; 144] {
    st.to_bytes()
}

/// seccomp_data's and SIGSYS's architecture: AUDIT_ARCH_X86_64 (EM_X86_64 with the 64-bit and little-endian bits).
pub const AUDIT_ARCH: u32 = 0xc000_003e;

/// struct epoll_event's data offset: packed on x86_64 (the kernel's EPOLL_PACKED).
pub const EPOLL_DATA_AT: usize = 4;

/// AT_PLATFORM's string on x86_64 (auxv_test).
pub const PLATFORM: &[u8] = b"x86_64";

/// AT_HWCAP: CPUID leaf 1's EDX on x86_64 (auxv_test).
pub fn hwcap() -> u64 {
    let (_, _, _, edx) = platform::cpuid(1, 0);
    u64::from(edx)
}

/// What rt_sigreturn finds at the stack pointer `sp` (the handler
/// returned to sa_restorer, which popped the return address: `sp` is the
/// ucontext): the registers, the FPU state (None: the frame has none)
/// and the signal mask the frame saved.
pub fn read_sigframe(
    process: &Process,
    sp: u64,
) -> Result<(UserRegs, Option<XState>, SigSet), &'static str> {
    let mut b = [0u8; abi::UCONTEXT_SIZE];
    process.read(sp, &mut b).map_err(|_| "the ucontext is not readable")?;
    let uc = Ucontext::from_bytes(&b);
    let fp = uc.uc_mcontext.fpstate;
    let fx = if fp != 0 {
        let mut fx = XState::initial();
        let len = xstate::size();
        process
            .read(fp, &mut fx.bytes_mut()[..len])
            .map_err(|_| "the FPU state is not readable")?;
        Some(fx)
    } else {
        None
    };
    Ok((regs_of(&uc.uc_mcontext), fx, SigSet(uc.uc_sigmask)))
}

/// Where rt_sigreturn finds the frame's uc_stack: 16 bytes into the
/// ucontext at its sp.
pub const UC_STACK_AT: u64 = 16;

/// Bytes below the stack pointer a function may use without moving it
/// (the psABI's red zone): a frame starts below them.
pub const RED_ZONE: u64 = 128;
/// RFLAGS.DF, clear on entry to a handler (the psABI's function entry).
const DF: u64 = 1 << 10;
/// RFLAGS.TF.
const TF: u64 = 1 << 8;

/// Where a handler's frame goes on a stack whose pointer is `sp`:
/// (the frame's address, where the FPU state goes). The FPU state
/// (`fp_len` bytes) sits 64-aligned below the red zone; the frame below
/// it, placed so that the handler starts as if called: rsp + 8 is
/// 16-aligned (rsp points at sa_restorer, its return address).
pub fn frame_layout(sp: u64, fp_len: u64) -> Option<(u64, u64)> {
    let fp = sp.checked_sub(RED_ZONE)?.checked_sub(fp_len)? & !63;
    let frame = (fp.checked_sub(abi::RT_SIGFRAME_SIZE as u64)? & !15).checked_sub(8)?;
    Some((frame, fp))
}

/// The registers a ucontext's signal context holds, in `UserRegs`'
/// order (rax, rcx, rdx, rbx, rsp, rbp, rsi, rdi, r8..r15).
fn sigcontext_of(regs: &UserRegs, old_mask: SigSet, fpstate: u64) -> Sigcontext {
    let g = regs.gprs();
    Sigcontext {
        rax: g[0],
        rcx: g[1],
        rdx: g[2],
        rbx: g[3],
        rsp: g[4],
        rbp: g[5],
        rsi: g[6],
        rdi: g[7],
        r8: g[8],
        r9: g[9],
        r10: g[10],
        r11: g[11],
        r12: g[12],
        r13: g[13],
        r14: g[14],
        r15: g[15],
        rip: regs.ip(),
        eflags: regs.flags(),
        cs: abi::USER_CS,
        ss: abi::USER_SS,
        oldmask: old_mask.0,
        fpstate,
        ..Sigcontext::default()
    }
}

/// The registers rt_sigreturn returns to from a saved signal context
/// (the flags are filtered to what user mode may set by the return).
pub fn regs_of(m: &Sigcontext) -> UserRegs {
    let gpr = [
        m.rax, m.rcx, m.rdx, m.rbx, m.rsp, m.rbp, m.rsi, m.rdi, m.r8, m.r9, m.r10, m.r11, m.r12,
        m.r13, m.r14, m.r15,
    ];
    UserRegs::new(gpr, m.rip, m.eflags)
}

/// A handler's frame, ready to be written at `addr`, and the registers
/// the handler starts with.
pub struct Frame {
    pub addr: u64,
    pub bytes: [u8; abi::RT_SIGFRAME_SIZE],
    /// Where the FPU state goes (the ucontext's fpstate), beside the frame.
    pub fpstate: u64,
    /// How much of it (xstate::size()).
    pub fp_len: usize,
    pub regs: UserRegs,
}

/// Builds the rt_sigframe for `sig` (`info`, handled by `action`) that
/// interrupts `regs`, whose mask was `old_mask`; the FPU state `_fx`
/// goes beside it (xstate::size() bytes at `fpstate`, which the caller
/// writes). None if the stack pointer leaves no room.
///
/// The handler starts with rip = sa_handler, rsp = the frame (its first
/// word: sa_restorer, the return address), rdi = sig, rsi = &siginfo,
/// rdx = &ucontext, rax 0, DF and TF clear, every other register as it
/// was. The frame goes below `sp` (the interrupted rsp, or the top of
/// the alternate stack). The ucontext: uc_flags UC_SIGCONTEXT_SS|
/// UC_STRICT_RESTORE_SS (signal_test), the alternate stack as
/// `uc_stack`, the interrupted mask in uc_sigmask and in oldmask.
#[allow(clippy::too_many_arguments)]
pub fn build_frame(
    regs: &UserRegs,
    sp: u64,
    uc_stack: Stack,
    sig: u32,
    info: &Siginfo,
    action: &KSigaction,
    old_mask: SigSet,
    _fx: &XState,
) -> Option<Frame> {
    let fp_len = xstate::size();
    let (addr, fpstate) = frame_layout(sp, fp_len as u64)?;
    let frame = RtSigframe {
        pretcode: action.restorer,
        uc: Ucontext {
            uc_flags: abi::UC_SIGCONTEXT_SS | abi::UC_STRICT_RESTORE_SS,
            uc_link: 0,
            uc_stack,
            uc_mcontext: sigcontext_of(regs, old_mask, fpstate),
            uc_sigmask: old_mask.0,
        },
        info: *info,
    };
    let uc = addr + core::mem::offset_of!(RtSigframe, uc) as u64;
    let si = addr + core::mem::offset_of!(RtSigframe, info) as u64;
    let mut handler = *regs;
    handler.set_ip(action.handler);
    handler.set_gpr(Rax, 0);
    handler.set_gpr(Rsp, addr);
    handler.set_gpr(Rdi, u64::from(sig));
    handler.set_gpr(Rsi, si);
    handler.set_gpr(Rdx, uc);
    handler.set_flags(handler.flags() & !(DF | TF));
    Some(Frame { addr, bytes: frame.to_bytes(), fpstate, fp_len, regs: handler })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lx_abi::signal::{SA_RESTORER, SA_SIGINFO, SIGPIPE, SIGUSR1};

    fn handler(h: u64, flags: u64, mask: u64) -> KSigaction {
        KSigaction { handler: h, flags: flags | SA_RESTORER, restorer: 0x40_1000, mask }
    }

    fn pipe_info() -> Siginfo {
        Siginfo::from_process(SIGPIPE as i32, abi::SI_USER, 4, 1000)
    }

    /// The frame as signal_test observes it on Linux: the restorer at the
    /// handler's rsp (rsp + 8 16-aligned, as after a call), the ucontext
    /// right above, siginfo 304 bytes further, the FPU state 64-aligned
    /// above the frame and below the red zone.
    #[test]
    fn a_handler_frame() {
        let mut regs = UserRegs::default();
        for (i, r) in regs.gprs_mut().iter_mut().enumerate() {
            *r = 0x100 + i as u64;
        }
        regs.set_sp(0x7fff_ffff_e123);
        regs.set_ip(0x40_2000);
        regs.set_flags(0x202 | DF);
        let a = handler(0x40_3000, SA_SIGINFO, 0);
        let info = pipe_info();
        let f = build_frame(
            &regs,
            regs.sp(),
            Stack { ss_sp: 0, ss_flags: abi::SS_DISABLE, ss_size: 0 },
            SIGPIPE,
            &info,
            &a,
            SigSet::of(SIGUSR1),
            &XState::initial(),
        )
        .unwrap();
        assert_eq!((f.addr + 8) % 16, 0);
        assert_eq!(f.fpstate % 64, 0);
        assert!(f.fpstate + f.fp_len as u64 <= regs.sp() - RED_ZONE);
        assert!(f.addr + abi::RT_SIGFRAME_SIZE as u64 <= f.fpstate);
        let r = &f.regs;
        assert_eq!((r.ip(), r.sp(), r.gpr(Rdi)), (0x40_3000, f.addr, 13));
        assert_eq!((r.gpr(Rdx), r.gpr(Rsi)), (f.addr + 8, f.addr + 312));
        assert_eq!((r.gpr(Rax), r.flags() & DF), (0, 0));
        assert_eq!(r.gpr(Rbx), regs.gpr(Rbx));
        assert_eq!(f.bytes[..8], 0x40_1000u64.to_le_bytes());
        let uc = Ucontext::from_bytes(f.bytes[8..8 + abi::UCONTEXT_SIZE].try_into().unwrap());
        assert_eq!(regs_of(&uc.uc_mcontext).gprs(), regs.gprs());
        assert_eq!(uc.uc_mcontext.rip, regs.ip());
        assert_eq!(uc.uc_mcontext.fpstate, f.fpstate);
        assert_eq!((uc.uc_sigmask, uc.uc_mcontext.oldmask), (1 << 9, 1 << 9));
        assert_eq!(uc.uc_flags, 6);
        assert_eq!(f.bytes[312..440], info.to_bytes());
        // No room below a tiny stack pointer.
        assert!(
            build_frame(
                &UserRegs::default(),
                0,
                Stack { ss_sp: 0, ss_flags: abi::SS_DISABLE, ss_size: 0 },
                SIGPIPE,
                &info,
                &a,
                SigSet::EMPTY,
                &XState::initial(),
            )
            .is_none()
        );
    }
}
