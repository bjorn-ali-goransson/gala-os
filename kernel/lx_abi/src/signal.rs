//! Signals as a Linux x86_64 program sees them: numbers, `sa_flags`,
//! si_code values, signal sets, default actions, and the layouts the
//! kernel writes when it delivers a signal (siginfo, stack_t, the signal
//! context, the ucontext, the rt_sigframe).
//!
//! Sources: signal(7), sigaction(2), sigaltstack(2), rt_sigreturn(2), the
//! System V AMD64 psABI, musl 1.2.4's `<signal.h>`/`<bits/signal.h>` (read
//! as data); checked against user/lx/signal_test's transcript recorded on
//! Linux 6.18, which observes the frame (a fault probe puts a marker in
//! every register and the handler prints where each lands in the
//! ucontext) and every signal's default action.

use core::mem::{offset_of, size_of};

pub const SIGHUP: u32 = 1;
pub const SIGINT: u32 = 2;
pub const SIGQUIT: u32 = 3;
pub const SIGILL: u32 = 4;
pub const SIGTRAP: u32 = 5;
pub const SIGABRT: u32 = 6;
pub const SIGBUS: u32 = 7;
pub const SIGFPE: u32 = 8;
pub const SIGKILL: u32 = 9;
pub const SIGUSR1: u32 = 10;
pub const SIGSEGV: u32 = 11;
pub const SIGUSR2: u32 = 12;
pub const SIGPIPE: u32 = 13;
pub const SIGALRM: u32 = 14;
pub const SIGTERM: u32 = 15;
pub const SIGSTKFLT: u32 = 16;
pub const SIGCHLD: u32 = 17;
pub const SIGCONT: u32 = 18;
pub const SIGSTOP: u32 = 19;
pub const SIGTSTP: u32 = 20;
pub const SIGTTIN: u32 = 21;
pub const SIGTTOU: u32 = 22;
pub const SIGURG: u32 = 23;
pub const SIGXCPU: u32 = 24;
pub const SIGXFSZ: u32 = 25;
pub const SIGVTALRM: u32 = 26;
pub const SIGPROF: u32 = 27;
pub const SIGWINCH: u32 = 28;
pub const SIGIO: u32 = 29;
pub const SIGPWR: u32 = 30;
pub const SIGSYS: u32 = 31;
/// The kernel's first real-time signal (C libraries reserve some: musl's
/// SIGRTMIN is 35).
pub const SIGRTMIN: u32 = 32;
pub const SIGRTMAX: u32 = 64;
/// Signals are 1..=NSIG.
pub const NSIG: u32 = 64;

/// The sigset size rt_sigaction/rt_sigprocmask/rt_sigpending accept
/// (anything else is EINVAL; signal_test).
pub const SIGSET_SIZE: usize = 8;

pub const SIG_DFL: u64 = 0;
pub const SIG_IGN: u64 = 1;

pub const SA_NOCLDSTOP: u64 = 0x1;
pub const SA_NOCLDWAIT: u64 = 0x2;
pub const SA_SIGINFO: u64 = 0x4;
pub const SA_RESTORER: u64 = 0x0400_0000;
pub const SA_ONSTACK: u64 = 0x0800_0000;
pub const SA_RESTART: u64 = 0x1000_0000;
pub const SA_NODEFER: u64 = 0x4000_0000;
pub const SA_RESETHAND: u64 = 0x8000_0000;

pub const SIG_BLOCK: u32 = 0;
pub const SIG_UNBLOCK: u32 = 1;
pub const SIG_SETMASK: u32 = 2;

pub const SS_ONSTACK: i32 = 1;
pub const SS_DISABLE: i32 = 2;
/// sigaltstack(2): a smaller stack is ENOMEM.
pub const MINSIGSTKSZ: u64 = 2048;

// si_code values (sigaction(2)).
pub const SI_USER: i32 = 0;
pub const SI_KERNEL: i32 = 0x80;
pub const SI_QUEUE: i32 = -1;
pub const SI_TIMER: i32 = -2;
pub const SI_TKILL: i32 = -6;
pub const SEGV_MAPERR: i32 = 1;
pub const SEGV_ACCERR: i32 = 2;
pub const CLD_EXITED: i32 = 1;
pub const CLD_KILLED: i32 = 2;
pub const CLD_DUMPED: i32 = 3;
pub const CLD_TRAPPED: i32 = 4;
pub const CLD_STOPPED: i32 = 5;
pub const CLD_CONTINUED: i32 = 6;

/// A valid signal number (1..=64).
pub fn valid(sig: u32) -> bool {
    (1..=NSIG).contains(&sig)
}

/// A set of signals: bit `sig - 1` for each signal, as the kernel's
/// 64-bit `sigset_t` holds it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SigSet(pub u64);

impl SigSet {
    pub const EMPTY: SigSet = SigSet(0);
    pub const FULL: SigSet = SigSet(!0);
    /// What can never be blocked, caught or ignored.
    pub const UNBLOCKABLE: SigSet = SigSet(1 << (SIGKILL - 1) | 1 << (SIGSTOP - 1));

    /// The set of one signal; empty for an invalid number.
    pub fn of(sig: u32) -> SigSet {
        if valid(sig) { SigSet(1 << (sig - 1)) } else { SigSet::EMPTY }
    }
    pub fn contains(self, sig: u32) -> bool {
        valid(sig) && self.0 & (1 << (sig - 1)) != 0
    }
    pub fn with(self, sig: u32) -> SigSet {
        SigSet(self.0 | SigSet::of(sig).0)
    }
    pub fn without(self, sig: u32) -> SigSet {
        SigSet(self.0 & !SigSet::of(sig).0)
    }
    pub fn union(self, other: SigSet) -> SigSet {
        SigSet(self.0 | other.0)
    }
    pub fn minus(self, other: SigSet) -> SigSet {
        SigSet(self.0 & !other.0)
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    /// What a blocked mask (rt_sigprocmask, sa_mask) becomes: SIGKILL and
    /// SIGSTOP silently dropped (signal_test: SIG_SETMASK of all reads back
    /// 0xfffffffffffbfeff).
    pub fn blockable(self) -> SigSet {
        self.minus(SigSet::UNBLOCKABLE)
    }
    /// rt_sigprocmask's `how` applied to `self` (the current mask); None
    /// for an unknown `how` (EINVAL).
    pub fn apply(self, how: u32, set: SigSet) -> Option<SigSet> {
        let new = match how {
            SIG_BLOCK => self.union(set),
            SIG_UNBLOCK => self.minus(set),
            SIG_SETMASK => set,
            _ => return None,
        };
        Some(new.blockable())
    }
    /// The lowest-numbered signal in the set.
    pub fn lowest(self) -> Option<u32> {
        if self.0 == 0 { None } else { Some(self.0.trailing_zeros() + 1) }
    }
}

/// What a signal does when its action is SIG_DFL (signal(7)).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DefaultAction {
    /// Terminate the process.
    Term,
    /// Terminate the process and dump core (when the limits allow).
    Core,
    /// Ignore the signal (it is discarded when sent, unless blocked).
    Ign,
    /// Stop the process (not in an orphaned process group, for SIGTSTP,
    /// SIGTTIN and SIGTTOU from a terminal).
    Stop,
    /// Continue the process if stopped.
    Cont,
}

/// signal(7)'s table; real-time signals terminate. None for an invalid
/// number.
pub fn default_action(sig: u32) -> Option<DefaultAction> {
    use DefaultAction::*;
    Some(match sig {
        SIGHUP | SIGINT | SIGKILL | SIGUSR1 | SIGUSR2 | SIGPIPE | SIGALRM | SIGTERM | SIGSTKFLT
        | SIGVTALRM | SIGPROF | SIGIO | SIGPWR => Term,
        SIGQUIT | SIGILL | SIGTRAP | SIGABRT | SIGBUS | SIGFPE | SIGSEGV | SIGXCPU | SIGXFSZ
        | SIGSYS => Core,
        SIGCHLD | SIGURG | SIGWINCH => Ign,
        SIGCONT => Cont,
        SIGSTOP | SIGTSTP | SIGTTIN | SIGTTOU => Stop,
        SIGRTMIN..=SIGRTMAX => Term,
        _ => return None,
    })
}

/// The kernel's `struct sigaction` for rt_sigaction(2) (not libc's): the
/// handler (or SIG_DFL/SIG_IGN), flags, the restorer (SA_RESTORER: the
/// code a handler returns to, which calls rt_sigreturn), the mask.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct KSigaction {
    pub handler: u64,
    pub flags: u64,
    pub restorer: u64,
    pub mask: u64,
}

/// `siginfo_t`: 128 bytes; signo, errno, code, then a union whose fields
/// sit at fixed offsets (see the `SI_*_OFFSET` constants). Kept as words
/// so it is plain data; the constructors fill the union for each kind of
/// signal.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Siginfo {
    pub signo: i32,
    pub errno: i32,
    pub code: i32,
    _pad: i32,
    /// The union, from offset 16.
    pub fields: [u32; 28],
}

/// Offsets in siginfo_t (from its start).
pub const SI_PID_OFFSET: usize = 16;
pub const SI_UID_OFFSET: usize = 20;
pub const SI_STATUS_OFFSET: usize = 24;
pub const SI_VALUE_OFFSET: usize = 24;
pub const SI_ADDR_OFFSET: usize = 16;

impl Siginfo {
    pub const fn zeroed(signo: i32, code: i32) -> Siginfo {
        Siginfo { signo, errno: 0, code, _pad: 0, fields: [0; 28] }
    }
    fn set_u32(&mut self, offset: usize, v: u32) {
        self.fields[(offset - 16) / 4] = v;
    }
    fn set_u64(&mut self, offset: usize, v: u64) {
        self.set_u32(offset, v as u32);
        self.set_u32(offset + 4, (v >> 32) as u32);
    }
    pub fn get_u32(&self, offset: usize) -> u32 {
        self.fields[(offset - 16) / 4]
    }
    pub fn get_u64(&self, offset: usize) -> u64 {
        self.get_u32(offset) as u64 | (self.get_u32(offset + 4) as u64) << 32
    }
    /// kill(2), tgkill(2), a pipe's SIGPIPE: the sender's pid and uid.
    pub fn from_process(signo: i32, code: i32, pid: i32, uid: u32) -> Siginfo {
        let mut s = Siginfo::zeroed(signo, code);
        s.set_u32(SI_PID_OFFSET, pid as u32);
        s.set_u32(SI_UID_OFFSET, uid);
        s
    }
    /// rt_sigqueueinfo/sigqueue (SI_QUEUE): pid, uid and the value.
    pub fn queued(signo: i32, pid: i32, uid: u32, value: u64) -> Siginfo {
        let mut s = Siginfo::from_process(signo, SI_QUEUE, pid, uid);
        s.set_u64(SI_VALUE_OFFSET, value);
        s
    }
    /// SIGCHLD: the child's pid, uid, and its exit status (CLD_EXITED) or
    /// signal (CLD_KILLED, CLD_DUMPED, CLD_STOPPED, CLD_CONTINUED).
    pub fn child(code: i32, pid: i32, uid: u32, status: i32) -> Siginfo {
        let mut s = Siginfo::from_process(SIGCHLD as i32, code, pid, uid);
        s.set_u32(SI_STATUS_OFFSET, status as u32);
        s
    }
    /// A fault (SIGSEGV, SIGBUS, SIGILL, SIGFPE, SIGTRAP): the address.
    pub fn fault(signo: i32, code: i32, addr: u64) -> Siginfo {
        let mut s = Siginfo::zeroed(signo, code);
        s.set_u64(SI_ADDR_OFFSET, addr);
        s
    }
}

/// `stack_t` (sigaltstack(2), the ucontext's uc_stack).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Stack {
    pub ss_sp: u64,
    pub ss_flags: i32,
    pub ss_size: u64,
}

/// The x86_64 signal context (the ucontext's uc_mcontext; libc's
/// `mcontext_t` with `gregs[23]`): the general registers in this order,
/// then rip, eflags, the segment selectors, the fault's error code, trap
/// number, the old mask, cr2, and the pointer to the saved FPU/XSAVE
/// state.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Sigcontext {
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub rdx: u64,
    pub rax: u64,
    pub rcx: u64,
    pub rsp: u64,
    pub rip: u64,
    pub eflags: u64,
    pub cs: u16,
    pub gs: u16,
    pub fs: u16,
    pub ss: u16,
    pub err: u64,
    pub trapno: u64,
    pub oldmask: u64,
    pub cr2: u64,
    pub fpstate: u64,
    pub reserved: [u64; 8],
}

/// uc_flags bits.
pub const UC_FP_XSTATE: u64 = 1;
pub const UC_SIGCONTEXT_SS: u64 = 2;
pub const UC_STRICT_RESTORE_SS: u64 = 4;

/// The kernel's ucontext (304 bytes: libc's ucontext_t is larger, with
/// a 128-byte sigset and room for the FPU state, but the kernel writes
/// only this, then the siginfo right after it).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Ucontext {
    pub uc_flags: u64,
    pub uc_link: u64,
    pub uc_stack: Stack,
    pub uc_mcontext: Sigcontext,
    pub uc_sigmask: u64,
}

/// The x86_64 rt_sigframe the kernel builds on the (alternate) stack: the
/// handler starts with rsp at `pretcode` (the restorer, its return
/// address), rsi = &info, rdx = &uc. The FPU/XSAVE state follows, 64-byte
/// aligned, at `uc.uc_mcontext.fpstate`; its size depends on the CPU.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RtSigframe {
    pub pretcode: u64,
    pub uc: Ucontext,
    pub info: Siginfo,
}

/// The selectors a 64-bit user frame holds (signal_test: cs 0x33).
pub const USER_CS: u16 = 0x33;
pub const USER_SS: u16 = 0x2b;

/// A page fault's trap number and error-code bits (signal_test: a user
/// read of an unmapped page is err 4, a user write to a read-only one 6).
pub const TRAP_PAGE_FAULT: u64 = 14;
pub const PF_PRESENT: u64 = 1;
pub const PF_WRITE: u64 = 2;
pub const PF_USER: u64 = 4;

/// Bytes of a `KSigaction`, a `siginfo_t`, the kernel's ucontext and the
/// rt_sigframe as user memory holds them.
pub const KSIGACTION_SIZE: usize = 32;
pub const SIGINFO_SIZE: usize = 128;
pub const UCONTEXT_SIZE: usize = 304;
pub const RT_SIGFRAME_SIZE: usize = 440;

/// Little-endian fields written at increasing offsets.
struct Out<'a> {
    buf: &'a mut [u8],
    at: usize,
}

impl Out<'_> {
    fn bytes(&mut self, b: &[u8]) {
        self.buf[self.at..self.at + b.len()].copy_from_slice(b);
        self.at += b.len();
    }
    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }
    fn u16(&mut self, v: u16) {
        self.bytes(&v.to_le_bytes());
    }
}

/// Little-endian fields read at increasing offsets.
struct In<'a> {
    buf: &'a [u8],
    at: usize,
}

impl In<'_> {
    fn take<const N: usize>(&mut self) -> [u8; N] {
        let mut b = [0; N];
        b.copy_from_slice(&self.buf[self.at..self.at + N]);
        self.at += N;
        b
    }
    fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.take())
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take())
    }
    fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.take())
    }
}

impl KSigaction {
    pub fn from_bytes(b: &[u8; KSIGACTION_SIZE]) -> KSigaction {
        let mut i = In { buf: b, at: 0 };
        KSigaction { handler: i.u64(), flags: i.u64(), restorer: i.u64(), mask: i.u64() }
    }
    pub fn to_bytes(&self) -> [u8; KSIGACTION_SIZE] {
        let mut b = [0; KSIGACTION_SIZE];
        let mut o = Out { buf: &mut b, at: 0 };
        for v in [self.handler, self.flags, self.restorer, self.mask] {
            o.u64(v);
        }
        b
    }
}

impl Siginfo {
    pub fn to_bytes(&self) -> [u8; SIGINFO_SIZE] {
        let mut b = [0; SIGINFO_SIZE];
        let mut o = Out { buf: &mut b, at: 0 };
        o.u32(self.signo as u32);
        o.u32(self.errno as u32);
        o.u32(self.code as u32);
        o.u32(0);
        for v in self.fields {
            o.u32(v);
        }
        b
    }
}

impl Sigcontext {
    fn write(&self, o: &mut Out<'_>) {
        for v in [
            self.r8,
            self.r9,
            self.r10,
            self.r11,
            self.r12,
            self.r13,
            self.r14,
            self.r15,
            self.rdi,
            self.rsi,
            self.rbp,
            self.rbx,
            self.rdx,
            self.rax,
            self.rcx,
            self.rsp,
            self.rip,
            self.eflags,
        ] {
            o.u64(v);
        }
        for v in [self.cs, self.gs, self.fs, self.ss] {
            o.u16(v);
        }
        for v in [self.err, self.trapno, self.oldmask, self.cr2, self.fpstate] {
            o.u64(v);
        }
        for v in self.reserved {
            o.u64(v);
        }
    }

    fn read(i: &mut In<'_>) -> Sigcontext {
        let mut g = [0u64; 18];
        for v in &mut g {
            *v = i.u64();
        }
        let [
            r8,
            r9,
            r10,
            r11,
            r12,
            r13,
            r14,
            r15,
            rdi,
            rsi,
            rbp,
            rbx,
            rdx,
            rax,
            rcx,
            rsp,
            rip,
            eflags,
        ] = g;
        let (cs, gs, fs, ss) = (i.u16(), i.u16(), i.u16(), i.u16());
        let (err, trapno, oldmask, cr2, fpstate) = (i.u64(), i.u64(), i.u64(), i.u64(), i.u64());
        let mut reserved = [0u64; 8];
        for v in &mut reserved {
            *v = i.u64();
        }
        Sigcontext {
            r8,
            r9,
            r10,
            r11,
            r12,
            r13,
            r14,
            r15,
            rdi,
            rsi,
            rbp,
            rbx,
            rdx,
            rax,
            rcx,
            rsp,
            rip,
            eflags,
            cs,
            gs,
            fs,
            ss,
            err,
            trapno,
            oldmask,
            cr2,
            fpstate,
            reserved,
        }
    }
}

impl Ucontext {
    fn write(&self, o: &mut Out<'_>) {
        o.u64(self.uc_flags);
        o.u64(self.uc_link);
        o.u64(self.uc_stack.ss_sp);
        o.u32(self.uc_stack.ss_flags as u32);
        o.u32(0);
        o.u64(self.uc_stack.ss_size);
        self.uc_mcontext.write(o);
        o.u64(self.uc_sigmask);
    }

    pub fn to_bytes(&self) -> [u8; UCONTEXT_SIZE] {
        let mut b = [0; UCONTEXT_SIZE];
        self.write(&mut Out { buf: &mut b, at: 0 });
        b
    }

    /// The ucontext rt_sigreturn finds on the user stack.
    pub fn from_bytes(b: &[u8; UCONTEXT_SIZE]) -> Ucontext {
        let mut i = In { buf: b, at: 0 };
        let (uc_flags, uc_link) = (i.u64(), i.u64());
        let ss_sp = i.u64();
        let ss_flags = i.u32() as i32;
        let _pad = i.u32();
        let ss_size = i.u64();
        let uc_mcontext = Sigcontext::read(&mut i);
        let uc_sigmask = i.u64();
        Ucontext {
            uc_flags,
            uc_link,
            uc_stack: Stack { ss_sp, ss_flags, ss_size },
            uc_mcontext,
            uc_sigmask,
        }
    }
}

impl RtSigframe {
    pub fn to_bytes(&self) -> [u8; RT_SIGFRAME_SIZE] {
        let mut b = [0; RT_SIGFRAME_SIZE];
        let mut o = Out { buf: &mut b, at: 0 };
        o.u64(self.pretcode);
        self.uc.write(&mut o);
        o.bytes(&self.info.to_bytes());
        b
    }
}

const _: () = {
    assert!(size_of::<KSigaction>() == 32);
    assert!(size_of::<Siginfo>() == 128);
    assert!(size_of::<Stack>() == 24);
    assert!(size_of::<Sigcontext>() == 256);
    assert!(size_of::<Ucontext>() == 304);
    assert!(offset_of!(RtSigframe, uc) == 8);
    assert!(offset_of!(RtSigframe, info) == 312);
    assert!(size_of::<KSigaction>() == KSIGACTION_SIZE);
    assert!(size_of::<Siginfo>() == SIGINFO_SIZE);
    assert!(size_of::<Ucontext>() == UCONTEXT_SIZE);
    assert!(size_of::<RtSigframe>() == RT_SIGFRAME_SIZE);
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{text, value};

    const SIGNAL: &str = include_str!("../../../user/lx/signal_test/expected.txt");
    const FORK: &str = include_str!("../../../user/lx/fork_test/expected.txt");

    fn uc_offset(key: &str) -> usize {
        value(SIGNAL, &format!("ucontext offset of {key}")) as usize
    }

    /// Where the fault probe's markers landed in the ucontext on Linux.
    #[test]
    fn ucontext_layout_matches_linux() {
        let m = offset_of!(Ucontext, uc_mcontext);
        let regs = [
            ("r8", offset_of!(Sigcontext, r8)),
            ("r9", offset_of!(Sigcontext, r9)),
            ("r10", offset_of!(Sigcontext, r10)),
            ("r11", offset_of!(Sigcontext, r11)),
            ("r12", offset_of!(Sigcontext, r12)),
            ("r13", offset_of!(Sigcontext, r13)),
            ("r14", offset_of!(Sigcontext, r14)),
            ("r15", offset_of!(Sigcontext, r15)),
            ("rdi", offset_of!(Sigcontext, rdi)),
            ("rsi", offset_of!(Sigcontext, rsi)),
            ("rbp", offset_of!(Sigcontext, rbp)),
            ("rbx", offset_of!(Sigcontext, rbx)),
            ("rdx", offset_of!(Sigcontext, rdx)),
            ("rax", offset_of!(Sigcontext, rax)),
        ];
        for (name, off) in regs {
            assert_eq!(uc_offset(name), m + off, "{name}");
        }
        assert_eq!(
            uc_offset("rsp (just above the handler's stack)"),
            m + offset_of!(Sigcontext, rsp)
        );
        assert_eq!(uc_offset("rip (the faulting instruction)"), m + offset_of!(Sigcontext, rip));
        assert_eq!(uc_offset("eflags (IF and bit 1 set)"), m + offset_of!(Sigcontext, eflags));
        assert_eq!(uc_offset("cs (0x33 in the low 16 bits)"), m + offset_of!(Sigcontext, cs));
        assert_eq!(uc_offset("err (4)"), m + offset_of!(Sigcontext, err));
        assert_eq!(uc_offset("trapno (14)"), m + offset_of!(Sigcontext, trapno));
        assert_eq!(uc_offset("the fpstate pointer"), m + offset_of!(Sigcontext, fpstate));
        assert_eq!(
            value(SIGNAL, "  ucontext offset of uc_stack.ss_sp") as usize,
            offset_of!(Ucontext, uc_stack)
        );
        // rcx and cr2 both held the fault address.
        let rcx = m + offset_of!(Sigcontext, rcx);
        let cr2 = m + offset_of!(Sigcontext, cr2);
        assert_eq!(
            text(SIGNAL, "ucontext offsets of the fault address (rcx, cr2)"),
            format!("{rcx} {cr2}")
        );
        // The interrupted mask: in oldmask and in uc_sigmask.
        let oldmask = m + offset_of!(Sigcontext, oldmask);
        let sigmask = offset_of!(Ucontext, uc_sigmask);
        assert_eq!(
            text(SIGNAL, "ucontext offsets of the interrupted mask (oldmask, uc_sigmask)"),
            format!("{oldmask} {sigmask}")
        );
        assert_eq!(
            value(SIGNAL, "musl offsetof(ucontext_t, uc_stack)") as usize,
            offset_of!(Ucontext, uc_stack)
        );
        assert_eq!(value(SIGNAL, "musl offsetof(ucontext_t, uc_mcontext)") as usize, m);
        assert_eq!(value(SIGNAL, "musl offsetof(ucontext_t, uc_sigmask)") as usize, sigmask);
        assert_eq!(value(SIGNAL, "sizeof(stack_t)") as usize, size_of::<Stack>());
    }

    /// The frame's shape: siginfo right after the ucontext, the restorer
    /// below it.
    #[test]
    fn frame_shape_matches_linux() {
        assert_eq!(
            value(SIGNAL, "siginfo minus ucontext (bytes)") as usize,
            offset_of!(RtSigframe, info) - offset_of!(RtSigframe, uc)
        );
        assert_eq!(text(SIGNAL, "the word below the ucontext is sa_restorer (pretcode)"), "yes");
        assert_eq!(offset_of!(RtSigframe, pretcode) + 8, offset_of!(RtSigframe, uc));
        assert_eq!(text(SIGNAL, "fpstate 64-aligned, after siginfo"), "yes");
        assert_eq!(value(SIGNAL, "cs") as u16, USER_CS);
        assert_eq!(value(SIGNAL, "trapno (page fault)") as u64, TRAP_PAGE_FAULT);
        assert_eq!(value(SIGNAL, "err (user read, not present)") as u64, PF_USER);
        assert_eq!(
            value(SIGNAL, "  err (user write, present)") as u64,
            PF_USER | PF_WRITE | PF_PRESENT
        );
        assert_eq!(
            value(SIGNAL, "uc_flags & (UC_SIGCONTEXT_SS|UC_STRICT_RESTORE_SS)") as u64,
            UC_SIGCONTEXT_SS | UC_STRICT_RESTORE_SS
        );
    }

    #[test]
    fn siginfo_layout_matches_linux() {
        assert_eq!(value(SIGNAL, "sizeof(siginfo_t)") as usize, size_of::<Siginfo>());
        assert_eq!(value(SIGNAL, "siginfo offset of si_addr") as usize, SI_ADDR_OFFSET);
        assert_eq!(value(SIGNAL, "  siginfo offset of the child's pid") as usize, SI_PID_OFFSET);
        assert_eq!(value(SIGNAL, "  siginfo offset of the exit status") as usize, SI_STATUS_OFFSET);
        assert_eq!(
            value(SIGNAL, "musl offsetof(siginfo_t, si_code)") as usize,
            offset_of!(Siginfo, code)
        );
        assert_eq!(value(SIGNAL, "musl offsetof(siginfo_t, si_pid)") as usize, SI_PID_OFFSET);
        assert_eq!(value(SIGNAL, "musl offsetof(siginfo_t, si_uid)") as usize, SI_UID_OFFSET);
        assert_eq!(value(SIGNAL, "musl offsetof(siginfo_t, si_status)") as usize, SI_STATUS_OFFSET);
        assert_eq!(value(SIGNAL, "musl offsetof(siginfo_t, si_value)") as usize, SI_VALUE_OFFSET);
        assert_eq!(offset_of!(Siginfo, fields), 16);

        let f = Siginfo::fault(SIGSEGV as i32, SEGV_MAPERR, 0xdead000);
        assert_eq!(f.get_u64(SI_ADDR_OFFSET), value(SIGNAL, "si_addr") as u64);
        assert_eq!(f.code, value(SIGNAL, "si_code (SEGV_MAPERR)") as i32);
        let c = Siginfo::child(CLD_EXITED, 1234, 0, 7);
        assert_eq!((c.get_u32(SI_PID_OFFSET), c.get_u32(SI_STATUS_OFFSET)), (1234, 7));
        assert_eq!(c.code, value(SIGNAL, "  si_code (CLD_EXITED)") as i32);
        let q = Siginfo::queued(SIGUSR1 as i32, 5, 6, 0x1234567);
        assert_eq!(q.get_u32(SI_VALUE_OFFSET) as i64, value(SIGNAL, "  si_value"));
        assert_eq!(q.code, value(SIGNAL, "  si_code (SI_QUEUE)") as i32);
        assert_eq!(SI_TKILL, value(SIGNAL, "  si_code (SI_TKILL)") as i32);
        assert_eq!(SI_USER, value(SIGNAL, "  si_code (SI_USER)") as i32);
        assert_eq!(SEGV_ACCERR, value(SIGNAL, "  si_code (SEGV_ACCERR)") as i32);
        assert_eq!(CLD_KILLED, value(FORK, "  si_code (CLD_KILLED)") as i32);
    }

    #[test]
    fn sigsets() {
        assert_eq!(
            SigSet::FULL.blockable().0,
            value(SIGNAL, "  now (SIGKILL and SIGSTOP stay unblocked)") as u64
        );
        let after =
            SigSet::FULL.blockable().apply(SIG_UNBLOCK, SigSet::of(SIGUSR1).with(SIGINT)).unwrap();
        assert_eq!(after.0, value(SIGNAL, "SIG_UNBLOCK(SIGUSR1|SIGINT)") as u64);
        let t = SigSet::FULL
            .apply(SIG_SETMASK, SigSet::EMPTY)
            .unwrap()
            .apply(SIG_BLOCK, SigSet::of(SIGTERM))
            .unwrap();
        assert_eq!(t.0, value(SIGNAL, "SIG_SETMASK(0), SIG_BLOCK(SIGTERM)") as u64);
        assert_eq!(SigSet::EMPTY.apply(3, SigSet::FULL), None);
        // A pending set as rt_sigpending showed it: USR1, USR2, musl's SIGRTMIN (35).
        let pending = SigSet::of(SIGUSR1).with(SIGUSR2).with(35);
        assert_eq!(
            pending.0,
            value(SIGNAL, "blocked, sent USR2, USR1 x2, RTMIN x2: rt_sigpending") as u64
        );
        assert_eq!(pending.lowest(), Some(SIGUSR1));
        assert!(pending.contains(35) && !pending.contains(36));
        assert_eq!(pending.without(SIGUSR1).lowest(), Some(SIGUSR2));
        assert!(SigSet::of(0).is_empty() && SigSet::of(65).is_empty());
        assert!(!SigSet::FULL.contains(0) && SigSet::FULL.contains(64));
        assert!(valid(1) && valid(64) && !valid(0) && !valid(65));
        // sa_mask is sanitized the same way (SIGUSR1's mask read back 0x800).
        let mask = SigSet::of(SIGKILL).with(SIGSTOP).with(SIGUSR2).blockable();
        assert_eq!(mask.0, 0x800);
        assert_eq!(SA_RESTORER | SA_SIGINFO | SA_RESTART, 0x1400_0004);
        assert!(text(SIGNAL, "SIGUSR1 now").contains("flags 0x14000004"));
        assert!(
            text(SIGNAL, "  SIGUSR2 after")
                .contains(&format!("flags {:#x}", SA_RESETHAND | SA_RESTORER | SA_SIGINFO))
        );
    }

    /// The bytes put each field at its `repr(C)` offset (which the tests
    /// above check against Linux), and read back the same.
    #[test]
    fn frames_encode_at_the_layout_offsets() {
        let mut m = Sigcontext::default();
        let fields: [&mut u64; 18] = [
            &mut m.r8,
            &mut m.r9,
            &mut m.r10,
            &mut m.r11,
            &mut m.r12,
            &mut m.r13,
            &mut m.r14,
            &mut m.r15,
            &mut m.rdi,
            &mut m.rsi,
            &mut m.rbp,
            &mut m.rbx,
            &mut m.rdx,
            &mut m.rax,
            &mut m.rcx,
            &mut m.rsp,
            &mut m.rip,
            &mut m.eflags,
        ];
        for (n, f) in fields.into_iter().enumerate() {
            *f = 0x1111 * (n as u64 + 1);
        }
        (m.cs, m.gs, m.fs, m.ss) = (USER_CS, 1, 2, USER_SS);
        (m.err, m.trapno, m.oldmask, m.cr2, m.fpstate) = (4, 14, 0x1000, 0xdead, 0x7fff_0040);
        m.reserved[7] = 9;
        let uc = Ucontext {
            uc_flags: UC_SIGCONTEXT_SS | UC_STRICT_RESTORE_SS,
            uc_link: 0x55,
            uc_stack: Stack { ss_sp: 0x7000, ss_flags: SS_DISABLE, ss_size: 0x2000 },
            uc_mcontext: m,
            uc_sigmask: 0x4000,
        };
        let info = Siginfo::from_process(SIGPIPE as i32, SI_USER, 77, 1000);
        let frame = RtSigframe { pretcode: 0x401000, uc, info };
        let b = frame.to_bytes();
        let u64_at = |off: usize| u64::from_le_bytes(b[off..off + 8].try_into().unwrap());
        let u32_at = |off: usize| u32::from_le_bytes(b[off..off + 4].try_into().unwrap());
        let mc = offset_of!(RtSigframe, uc) + offset_of!(Ucontext, uc_mcontext);
        assert_eq!(u64_at(offset_of!(RtSigframe, pretcode)), 0x401000);
        assert_eq!(u64_at(mc + offset_of!(Sigcontext, r8)), 0x1111);
        assert_eq!(u64_at(mc + offset_of!(Sigcontext, rax)), 0x1111 * 14);
        assert_eq!(u64_at(mc + offset_of!(Sigcontext, eflags)), 0x1111 * 18);
        assert_eq!(b[mc + offset_of!(Sigcontext, cs)], USER_CS as u8);
        assert_eq!(b[mc + offset_of!(Sigcontext, ss)], USER_SS as u8);
        assert_eq!(u64_at(mc + offset_of!(Sigcontext, oldmask)), 0x1000);
        assert_eq!(u64_at(mc + offset_of!(Sigcontext, fpstate)), 0x7fff_0040);
        assert_eq!(u64_at(mc + offset_of!(Sigcontext, reserved) + 56), 9);
        let ucb = offset_of!(RtSigframe, uc);
        assert_eq!(u64_at(ucb + offset_of!(Ucontext, uc_sigmask)), 0x4000);
        assert_eq!(u64_at(ucb + offset_of!(Ucontext, uc_link)), 0x55);
        let st = ucb + offset_of!(Ucontext, uc_stack);
        assert_eq!(u32_at(st + offset_of!(Stack, ss_flags)) as i32, SS_DISABLE);
        assert_eq!(u64_at(st + offset_of!(Stack, ss_size)), 0x2000);
        let si = offset_of!(RtSigframe, info);
        assert_eq!(u32_at(si + offset_of!(Siginfo, signo)), SIGPIPE);
        assert_eq!(u32_at(si + offset_of!(Siginfo, code)), SI_USER as u32);
        assert_eq!(u32_at(si + SI_PID_OFFSET), 77);
        assert_eq!(u32_at(si + SI_UID_OFFSET), 1000);
        let back = Ucontext::from_bytes(b[ucb..ucb + UCONTEXT_SIZE].try_into().unwrap());
        assert_eq!(back, uc);
        let sa = KSigaction { handler: 1, flags: SA_SIGINFO | SA_RESTORER, restorer: 3, mask: 4 };
        assert_eq!(KSigaction::from_bytes(&sa.to_bytes()), sa);
        assert_eq!(sa.to_bytes()[8..16], (SA_SIGINFO | SA_RESTORER).to_le_bytes());
    }

    /// signal_test sent every signal to a child left at SIG_DFL.
    #[test]
    fn default_actions_match_linux() {
        let mut checked = 0;
        for line in SIGNAL.lines() {
            let Some(rest) = line.strip_prefix("signal ") else { continue };
            let Some((num, outcome)) = rest.split_once(": ") else { continue };
            let sig: u32 = num.parse().unwrap();
            let action = default_action(sig).unwrap();
            let want = match action {
                DefaultAction::Term | DefaultAction::Core => format!("killed by {sig}"),
                DefaultAction::Stop => format!("stopped by {sig}"),
                DefaultAction::Ign | DefaultAction::Cont => "ignored (the child exited 0)".into(),
            };
            assert_eq!(outcome, want, "signal {sig}: {action:?}");
            checked += 1;
        }
        assert_eq!(checked, 33);
        assert_eq!(default_action(0), None);
        assert_eq!(default_action(65), None);
        assert_eq!(default_action(SIGSEGV), Some(DefaultAction::Core));
        assert_eq!(default_action(SIGCONT), Some(DefaultAction::Cont));
        assert_eq!(default_action(SIGTERM), Some(DefaultAction::Term));
    }
}
