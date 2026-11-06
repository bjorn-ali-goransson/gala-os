//! Linux signals (ROADMAP LX milestone 2): a process's actions, its
//! pending signals and the blocked mask, what delivering one does, and
//! the frame a handler runs on. Plain data and pure functions, host
//! tested; `lx::dispatch` makes the system calls and the delivery out of
//! them, the layouts and numbers are `lx_abi::signal`'s.
//!
//! Sources (CLEAN_ROOM.md "Linux"): signal(7), sigaction(2),
//! sigprocmask(2), sigpending(2), sigtimedwait(2), rt_sigreturn(2),
//! wait(2), the System V AMD64 psABI (the stack's alignment at a call,
//! the 128-byte red zone), and user/lx's transcripts recorded on Linux
//! (signal_test observes the frame; pipe_test SIGPIPE's siginfo).
//!
//! Model:
//! - The actions (rt_sigaction) are the process's; the blocked mask and
//!   the thread-directed pending set (SIGPIPE, faults, tgkill) are a
//!   thread's (`ThreadSignals`), the process-directed pending set
//!   (kill, SIGCHLD) the process's. A process has one thread today
//!   (clone of threads is not implemented), so `Signals` holds one
//!   `ThreadSignals`; with threads it becomes one per thread.
//! - A standard signal (1..=31) is pending at most once (a second one is
//!   merged); a real-time one queues each siginfo.
//! - Generation discards a signal whose action ignores it (SIG_IGN, or
//!   SIG_DFL with an Ign default) unless it is blocked: a blocked one
//!   stays pending (signal_test: "blocked and ignored: left pending").
//!   Setting an ignoring action discards what is pending.
//! - Delivery happens on the way back to user mode from a system call
//!   (`lx::dispatch`), the lowest-numbered deliverable signal first, one
//!   handler frame per return: the next one is delivered when its
//!   rt_sigreturn returns.

use alloc::vec::Vec;

use lx_abi::signal::{
    self as abi, DefaultAction, KSigaction, RtSigframe, SA_NODEFER, SA_RESETHAND, SIG_DFL, SIG_IGN,
    SigSet, Sigcontext, Siginfo, Stack, Ucontext,
};

use crate::mm::heap::TryVec;
use crate::nt::platform::UserRegs;
use crate::nt::status::NtStatus;

/// A signal's name for logs (signal(7)); real-time ones are "SIGRT".
pub fn name(sig: u32) -> &'static str {
    const NAMES: [&str; 31] = [
        "SIGHUP",
        "SIGINT",
        "SIGQUIT",
        "SIGILL",
        "SIGTRAP",
        "SIGABRT",
        "SIGBUS",
        "SIGFPE",
        "SIGKILL",
        "SIGUSR1",
        "SIGSEGV",
        "SIGUSR2",
        "SIGPIPE",
        "SIGALRM",
        "SIGTERM",
        "SIGSTKFLT",
        "SIGCHLD",
        "SIGCONT",
        "SIGSTOP",
        "SIGTSTP",
        "SIGTTIN",
        "SIGTTOU",
        "SIGURG",
        "SIGXCPU",
        "SIGXFSZ",
        "SIGVTALRM",
        "SIGPROF",
        "SIGWINCH",
        "SIGIO",
        "SIGPWR",
        "SIGSYS",
    ];
    match sig {
        1..=31 => NAMES[sig as usize - 1],
        abi::SIGRTMIN..=abi::SIGRTMAX => "SIGRT",
        _ => "?",
    }
}

/// What a pending set did with a new signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Raised {
    /// Ignored and not blocked: gone.
    Discarded,
    /// Now pending.
    Pending,
    /// A standard signal already pending: the new one merged into it.
    Merged,
}

/// Pending signals and their siginfo, in the order they came.
#[derive(Default)]
pub struct Pending {
    pub set: SigSet,
    queue: Vec<Siginfo>,
}

impl Pending {
    /// Adds `info`. A standard signal already pending is not queued
    /// again. Without memory for the siginfo the signal is still pending
    /// (it is then delivered with an SI_KERNEL siginfo).
    fn add(&mut self, info: Siginfo) -> Raised {
        let sig = info.signo as u32;
        if sig < abi::SIGRTMIN && self.set.contains(sig) {
            return Raised::Merged;
        }
        let _ = self.queue.try_push(info);
        self.set = self.set.with(sig);
        Raised::Pending
    }

    /// Takes the oldest siginfo of `sig`; its bit stays while more are
    /// queued (real-time signals).
    fn take(&mut self, sig: u32) -> Option<Siginfo> {
        if !self.set.contains(sig) {
            return None;
        }
        let at = self.queue.iter().position(|i| i.signo as u32 == sig);
        let info = match at {
            Some(i) => self.queue.remove(i),
            None => Siginfo::zeroed(sig as i32, abi::SI_KERNEL),
        };
        if !self.queue.iter().any(|i| i.signo as u32 == sig) {
            self.set = self.set.without(sig);
        }
        Some(info)
    }

    fn discard(&mut self, sig: u32) {
        self.set = self.set.without(sig);
        self.queue.retain(|i| i.signo as u32 != sig);
    }
}

/// A thread's part: its blocked mask and the signals sent to it.
#[derive(Default)]
pub struct ThreadSignals {
    pub mask: SigSet,
    pub pending: Pending,
    /// The mask rt_sigsuspend replaced: the handler frame of the signal
    /// that ends the wait saves it (uc_sigmask, put back by
    /// rt_sigreturn), or it is put back at once if no handler runs.
    pub saved_mask: Option<SigSet>,
}

/// What delivering a signal does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    Ignore,
    /// The process ends, killed by the signal (`core`: the default
    /// action dumps core; gala writes no core file).
    Terminate {
        core: bool,
    },
    Stop,
    Continue,
    Handler(KSigaction),
}

/// A process's signal state.
pub struct Signals {
    /// Signal n's action at n - 1.
    actions: [KSigaction; abi::NSIG as usize],
    /// Process-directed pending signals (kill, SIGCHLD).
    pub process: Pending,
    /// The one thread's mask and pending signals (see the module doc).
    pub thread: ThreadSignals,
}

impl Default for Signals {
    fn default() -> Self {
        Signals {
            actions: [KSigaction::default(); abi::NSIG as usize],
            process: Pending::default(),
            thread: ThreadSignals::default(),
        }
    }
}

impl Signals {
    /// A forked child's state: the same actions and mask, nothing pending
    /// (fork(2)).
    pub fn forked(&self) -> Signals {
        Signals {
            actions: self.actions,
            process: Pending::default(),
            thread: ThreadSignals {
                mask: self.thread.mask,
                pending: Pending::default(),
                saved_mask: None,
            },
        }
    }

    /// rt_sigsuspend: `mask` (without SIGKILL/SIGSTOP) until a signal is
    /// delivered; the mask it replaces is kept to put back.
    pub fn suspend(&mut self, mask: SigSet) {
        let old = core::mem::replace(&mut self.thread.mask, mask.blockable());
        self.thread.saved_mask.get_or_insert(old);
    }

    /// The mask a handler frame saves (rt_sigsuspend's, if it replaced
    /// one: taken), given the one in effect before the handler (`old`).
    pub fn frame_mask(&mut self, old: SigSet) -> SigSet {
        self.thread.saved_mask.take().unwrap_or(old)
    }

    /// No handler ran after rt_sigsuspend: its mask goes back.
    pub fn restore_saved_mask(&mut self) {
        if let Some(m) = self.thread.saved_mask.take() {
            self.thread.mask = m;
        }
    }

    /// After execve: caught signals go back to their default action
    /// (their handlers are gone with the old image), ignored ones stay
    /// ignored; the mask and what is pending stay (execve(2)).
    pub fn exec(&mut self) {
        for a in self.actions.iter_mut() {
            if a.handler != SIG_IGN && a.handler != SIG_DFL {
                *a = KSigaction::default();
            }
        }
    }

    /// Signal `sig`'s action (`sig` valid).
    pub fn action(&self, sig: u32) -> KSigaction {
        self.actions[sig as usize - 1]
    }

    /// rt_sigaction's change (the caller checked `sig` may change):
    /// returns the old action. sa_mask never holds SIGKILL or SIGSTOP; an
    /// action that ignores the signal discards what is pending of it.
    pub fn set_action(&mut self, sig: u32, mut new: KSigaction) -> KSigaction {
        new.mask = SigSet(new.mask).blockable().0;
        let old = core::mem::replace(&mut self.actions[sig as usize - 1], new);
        if self.ignored(sig) {
            self.process.discard(sig);
            self.thread.pending.discard(sig);
        }
        old
    }

    /// Whether `sig`'s action ignores it (SIG_IGN, or SIG_DFL whose
    /// default is to ignore).
    pub fn ignored(&self, sig: u32) -> bool {
        self.disposition(sig) == Disposition::Ignore
    }

    pub fn disposition(&self, sig: u32) -> Disposition {
        let action = self.action(sig);
        match action.handler {
            SIG_IGN => Disposition::Ignore,
            SIG_DFL => match abi::default_action(sig) {
                Some(DefaultAction::Term) | None => Disposition::Terminate { core: false },
                Some(DefaultAction::Core) => Disposition::Terminate { core: true },
                Some(DefaultAction::Ign) => Disposition::Ignore,
                Some(DefaultAction::Stop) => Disposition::Stop,
                Some(DefaultAction::Cont) => Disposition::Continue,
            },
            _ => Disposition::Handler(action),
        }
    }

    /// rt_sigprocmask: the old mask, or None for an unknown `how`.
    pub fn procmask(&mut self, how: u32, set: SigSet) -> Option<SigSet> {
        let new = self.thread.mask.apply(how, set)?;
        Some(core::mem::replace(&mut self.thread.mask, new))
    }

    /// A signal is generated: for the thread (`to_thread`: SIGPIPE,
    /// faults, tgkill) or the process (kill, SIGCHLD).
    pub fn raise(&mut self, info: Siginfo, to_thread: bool) -> Raised {
        let sig = info.signo as u32;
        if !abi::valid(sig) {
            return Raised::Discarded;
        }
        if !self.thread.mask.contains(sig) && self.ignored(sig) {
            return Raised::Discarded;
        }
        if to_thread { self.thread.pending.add(info) } else { self.process.add(info) }
    }

    /// Everything pending for the thread (its own and the process's).
    pub fn pending(&self) -> SigSet {
        self.thread.pending.set.union(self.process.set)
    }

    /// rt_sigpending: what is pending and blocked.
    pub fn pending_blocked(&self) -> SigSet {
        SigSet(self.pending().0 & self.thread.mask.0)
    }

    /// The signal to deliver next: the lowest pending one not blocked.
    pub fn deliverable(&self) -> Option<u32> {
        self.pending().minus(self.thread.mask).lowest()
    }

    /// Takes the lowest pending signal in `set` (rt_sigtimedwait), the
    /// thread's own first.
    pub fn take_from(&mut self, set: SigSet) -> Option<Siginfo> {
        let sig = SigSet(self.pending().0 & set.0).lowest()?;
        self.thread.pending.take(sig).or_else(|| self.process.take(sig))
    }

    /// A handler of `sig` is entered: the mask it runs with (the current
    /// one, its sa_mask, and `sig` itself unless SA_NODEFER); SA_RESETHAND
    /// puts the handler back to SIG_DFL (the flags, mask and restorer
    /// stay: signal_test). Returns the mask to restore on rt_sigreturn.
    pub fn enter_handler(&mut self, sig: u32, action: &KSigaction) -> SigSet {
        let old = self.thread.mask;
        let mut mask = old.union(SigSet(action.mask));
        if action.flags & SA_NODEFER == 0 {
            mask = mask.with(sig);
        }
        self.thread.mask = mask.blockable();
        if action.flags & SA_RESETHAND != 0 {
            self.actions[sig as usize - 1].handler = SIG_DFL;
        }
        old
    }

    /// rt_sigreturn puts back the interrupted mask.
    pub fn restore_mask(&mut self, mask: SigSet) {
        self.thread.mask = mask.blockable();
    }
}

/// The exit status of a Linux process killed by signal `sig`, as it
/// travels through the core (`nt::process::terminate`, the process's
/// exit status): an NTSTATUS in a customer range (severity error,
/// customer bit, facility "LX" 0x4c58) with the signal in the low byte.
/// `killed_by` reads it back.
pub fn killed_status(sig: u32) -> NtStatus {
    NtStatus(KILLED_BASE | (sig & 0xff))
}

const KILLED_BASE: u32 = 0xe4c5_8000;

/// The signal that ended a process with `status`: one sent to it
/// (`killed_status`), or the one a CPU exception stands for; None for an
/// exit status (exit_group's argument & 0xff).
pub fn killed_by(status: NtStatus) -> Option<u32> {
    let s = status.0;
    if s <= 0xff {
        return None;
    }
    if s & !0xff == KILLED_BASE {
        return Some(s & 0xff);
    }
    Some(match status {
        NtStatus::ACCESS_VIOLATION | NtStatus::STACK_OVERFLOW => abi::SIGSEGV,
        NtStatus::ILLEGAL_INSTRUCTION => abi::SIGILL,
        NtStatus::INTEGER_DIVIDE_BY_ZERO => abi::SIGFPE,
        NtStatus::BREAKPOINT => abi::SIGTRAP,
        // Ended from outside (another personality's terminate).
        _ => abi::SIGKILL,
    })
}

/// wait4's status word for a process that ended with `status` (wait(2)'s
/// macros: an exit code in bits 8..16, a terminating signal in the low 7
/// bits; gala writes no core file, so WCOREDUMP's bit is never set).
pub fn wait_status(status: NtStatus) -> u32 {
    match killed_by(status) {
        Some(sig) => sig & 0x7f,
        None => (status.0 & 0xff) << 8,
    }
}

/// The si_code and si_status of the SIGCHLD a parent gets for a child
/// that ended with `status`.
pub fn child_code(status: NtStatus) -> (i32, i32) {
    match killed_by(status) {
        Some(sig) => (abi::CLD_KILLED, sig as i32),
        None => (abi::CLD_EXITED, (status.0 & 0xff) as i32),
    }
}

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
    let g = &regs.gpr;
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
        rip: regs.rip,
        eflags: regs.rflags,
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
    UserRegs {
        gpr: [
            m.rax, m.rcx, m.rdx, m.rbx, m.rsp, m.rbp, m.rsi, m.rdi, m.r8, m.r9, m.r10, m.r11,
            m.r12, m.r13, m.r14, m.r15,
        ],
        rip: m.rip,
        rflags: m.eflags,
    }
}

/// A handler's frame, ready to be written at `addr`, and the registers
/// the handler starts with.
pub struct Frame {
    pub addr: u64,
    pub bytes: [u8; abi::RT_SIGFRAME_SIZE],
    /// Where the FPU state goes (the ucontext's fpstate).
    pub fpstate: u64,
    pub regs: UserRegs,
}

/// Builds the rt_sigframe for `sig` (`info`, handled by `action`) that
/// interrupts `regs`, whose mask was `old_mask`; `fp_len` bytes of FPU
/// state go with it. None if the stack pointer leaves no room.
///
/// The handler starts with rip = sa_handler, rsp = the frame (its first
/// word: sa_restorer, the return address), rdi = sig, rsi = &siginfo,
/// rdx = &ucontext, rax 0, DF and TF clear, every other register as it
/// was. The ucontext: uc_flags UC_SIGCONTEXT_SS|UC_STRICT_RESTORE_SS
/// (signal_test), no alternate stack (ss_flags SS_DISABLE), the
/// interrupted mask in uc_sigmask and in oldmask.
pub fn build_frame(
    regs: &UserRegs,
    sig: u32,
    info: &Siginfo,
    action: &KSigaction,
    old_mask: SigSet,
    fp_len: u64,
) -> Option<Frame> {
    let (addr, fpstate) = frame_layout(regs.gpr[4], fp_len)?;
    let frame = RtSigframe {
        pretcode: action.restorer,
        uc: Ucontext {
            uc_flags: abi::UC_SIGCONTEXT_SS | abi::UC_STRICT_RESTORE_SS,
            uc_link: 0,
            uc_stack: Stack { ss_sp: 0, ss_flags: abi::SS_DISABLE, ss_size: 0 },
            uc_mcontext: sigcontext_of(regs, old_mask, fpstate),
            uc_sigmask: old_mask.0,
        },
        info: *info,
    };
    let uc = addr + core::mem::offset_of!(RtSigframe, uc) as u64;
    let si = addr + core::mem::offset_of!(RtSigframe, info) as u64;
    let mut handler = *regs;
    handler.rip = action.handler;
    handler.gpr[0] = 0;
    handler.gpr[4] = addr;
    handler.gpr[7] = u64::from(sig);
    handler.gpr[6] = si;
    handler.gpr[2] = uc;
    handler.rflags &= !(DF | TF);
    Some(Frame { addr, bytes: frame.to_bytes(), fpstate, regs: handler })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lx_abi::signal::{SA_RESTORER, SA_SIGINFO, SIGCHLD, SIGKILL, SIGPIPE, SIGUSR1, SIGUSR2};

    fn handler(h: u64, flags: u64, mask: u64) -> KSigaction {
        KSigaction { handler: h, flags: flags | SA_RESTORER, restorer: 0x40_1000, mask }
    }

    fn pipe_info() -> Siginfo {
        Siginfo::from_process(SIGPIPE as i32, abi::SI_USER, 4, 1000)
    }

    /// pipe_test on Linux: a blocked SIGPIPE stays pending and
    /// rt_sigtimedwait takes it (si_code 0); ignored, it is discarded.
    #[test]
    fn sigpipe_blocked_ignored_and_taken() {
        let mut s = Signals::default();
        let pipe = SigSet::of(SIGPIPE);
        s.procmask(abi::SIG_BLOCK, pipe).unwrap();
        assert_eq!(s.raise(pipe_info(), true), Raised::Pending);
        assert_eq!(s.raise(pipe_info(), true), Raised::Merged);
        assert_eq!(s.pending_blocked(), pipe);
        assert_eq!(s.deliverable(), None);
        let info = s.take_from(pipe).unwrap();
        assert_eq!((info.signo, info.code), (SIGPIPE as i32, 0));
        assert!(s.pending().is_empty());
        s.procmask(abi::SIG_SETMASK, SigSet::EMPTY).unwrap();
        s.set_action(SIGPIPE, KSigaction { handler: SIG_IGN, ..Default::default() });
        assert_eq!(s.raise(pipe_info(), true), Raised::Discarded);
        // At SIG_DFL it is delivered and terminates.
        s.set_action(SIGPIPE, KSigaction::default());
        assert_eq!(s.raise(pipe_info(), true), Raised::Pending);
        assert_eq!(s.deliverable(), Some(SIGPIPE));
        assert_eq!(s.disposition(SIGPIPE), Disposition::Terminate { core: false });
    }

    /// signal_test on Linux: a blocked, ignored signal is left pending;
    /// setting SIG_IGN discards a pending one; SIGCHLD's default ignores.
    #[test]
    fn ignoring() {
        let mut s = Signals::default();
        assert_eq!(s.raise(Siginfo::child(abi::CLD_EXITED, 5, 0, 0), false), Raised::Discarded);
        s.set_action(SIGUSR2, KSigaction { handler: SIG_IGN, ..Default::default() });
        s.procmask(abi::SIG_BLOCK, SigSet::of(SIGUSR2)).unwrap();
        let usr2 = Siginfo::from_process(SIGUSR2 as i32, abi::SI_USER, 4, 1000);
        assert_eq!(s.raise(usr2, false), Raised::Pending);
        assert_eq!(s.pending_blocked(), SigSet::of(SIGUSR2));
        s.set_action(SIGUSR1, handler(0x1234, 0, 0));
        s.raise(Siginfo::from_process(SIGUSR1 as i32, 0, 4, 1000), false);
        s.set_action(SIGUSR2, KSigaction { handler: SIG_IGN, ..Default::default() });
        assert_eq!(s.pending(), SigSet::of(SIGUSR1));
    }

    #[test]
    fn real_time_signals_queue() {
        let mut s = Signals::default();
        s.procmask(abi::SIG_SETMASK, SigSet::FULL).unwrap();
        for v in [1, 2] {
            s.raise(Siginfo::queued(35, 4, 1000, v), false);
        }
        assert_eq!(s.take_from(SigSet::of(35)).unwrap().get_u64(abi::SI_VALUE_OFFSET), 1);
        assert!(s.pending().contains(35));
        assert_eq!(s.take_from(SigSet::of(35)).unwrap().get_u64(abi::SI_VALUE_OFFSET), 2);
        assert!(s.pending().is_empty());
        assert!(s.take_from(SigSet::FULL).is_none());
    }

    /// signal_test on Linux: the handler runs with its signal and sa_mask
    /// blocked (not with SA_NODEFER); SA_RESETHAND leaves SIG_DFL with
    /// the flags; SIGKILL never enters a mask.
    #[test]
    fn entering_a_handler() {
        let mut s = Signals::default();
        let a = handler(0x1234, SA_SIGINFO, SigSet::of(abi::SIGTERM).with(SIGKILL).0);
        s.set_action(SIGUSR2, a);
        assert_eq!(s.action(SIGUSR2).mask, SigSet::of(abi::SIGTERM).0);
        let a = s.action(SIGUSR2);
        let old = s.enter_handler(SIGUSR2, &a);
        assert_eq!(old, SigSet::EMPTY);
        assert_eq!(s.thread.mask, SigSet::of(SIGUSR2).with(abi::SIGTERM));
        s.restore_mask(old);
        let a = handler(0x1234, SA_SIGINFO | SA_NODEFER, 0);
        s.enter_handler(SIGUSR2, &a);
        assert!(!s.thread.mask.contains(SIGUSR2));
        s.restore_mask(SigSet::EMPTY);
        let a = handler(0x1234, SA_SIGINFO | SA_RESETHAND, 0);
        s.set_action(SIGUSR2, a);
        s.enter_handler(SIGUSR2, &a);
        assert!(s.thread.mask.contains(SIGUSR2));
        let after = s.action(SIGUSR2);
        assert_eq!((after.handler, after.flags), (SIG_DFL, 0x8400_0004));
        // A fork keeps actions and mask, not what is pending.
        s.raise(Siginfo::from_process(SIGUSR1 as i32, 0, 4, 1000), false);
        let child = s.forked();
        assert!(child.pending().is_empty());
        assert_eq!(child.thread.mask, s.thread.mask);
        assert_eq!(child.action(SIGUSR2), s.action(SIGUSR2));
    }

    /// fork_test on Linux: after execve a caught signal is at SIG_DFL, an
    /// ignored one still ignored, a blocked one still blocked.
    #[test]
    fn execve_resets_handlers() {
        let mut s = Signals::default();
        s.set_action(SIGUSR1, handler(0x1234, SA_SIGINFO, 0));
        s.set_action(SIGUSR2, KSigaction { handler: SIG_IGN, ..Default::default() });
        s.procmask(abi::SIG_BLOCK, SigSet::of(abi::SIGTERM)).unwrap();
        s.exec();
        assert_eq!(s.action(SIGUSR1), KSigaction::default());
        assert_eq!(s.action(SIGUSR2).handler, SIG_IGN);
        assert!(s.thread.mask.contains(abi::SIGTERM));
    }

    /// signal_test on Linux: rt_sigsuspend's mask is in effect until a
    /// handler runs; the handler's frame saves the mask from before it.
    #[test]
    fn suspend_saves_the_mask_for_the_frame() {
        let mut s = Signals::default();
        s.set_action(SIGUSR1, handler(0x1234, 0, 0));
        s.procmask(abi::SIG_SETMASK, SigSet::of(SIGUSR1)).unwrap();
        s.suspend(SigSet::of(SIGKILL).with(abi::SIGTERM));
        assert_eq!(s.thread.mask, SigSet::of(abi::SIGTERM));
        s.raise(Siginfo::from_process(SIGUSR1 as i32, 0, 4, 1000), false);
        assert_eq!(s.deliverable(), Some(SIGUSR1));
        let a = s.action(SIGUSR1);
        let before = s.enter_handler(SIGUSR1, &a);
        assert_eq!(s.frame_mask(before), SigSet::of(SIGUSR1));
        assert_eq!(s.thread.saved_mask, None);
        // Without a handler the old mask goes back at once.
        s.restore_mask(SigSet::of(SIGUSR1));
        s.suspend(SigSet::EMPTY);
        s.restore_saved_mask();
        assert_eq!(s.thread.mask, SigSet::of(SIGUSR1));
    }

    #[test]
    fn exit_and_kill_statuses() {
        assert_eq!(killed_by(NtStatus(0)), None);
        assert_eq!(killed_by(NtStatus(255)), None);
        assert_eq!(killed_by(killed_status(SIGPIPE)), Some(SIGPIPE));
        assert_eq!(killed_by(killed_status(64)), Some(64));
        assert_eq!(killed_by(NtStatus::ACCESS_VIOLATION), Some(abi::SIGSEGV));
        // wait(2)'s macros, as pipe_test's put_status reads them.
        assert_eq!(wait_status(NtStatus(7)), 7 << 8);
        assert_eq!(wait_status(killed_status(SIGPIPE)), 13);
        assert_eq!(child_code(killed_status(SIGPIPE)), (abi::CLD_KILLED, 13));
        assert_eq!(child_code(NtStatus(3)), (abi::CLD_EXITED, 3));
        assert_eq!(
            (name(SIGPIPE), name(SIGCHLD), name(abi::SIGSYS)),
            ("SIGPIPE", "SIGCHLD", "SIGSYS")
        );
        assert_eq!((name(40), name(0)), ("SIGRT", "?"));
    }

    /// The frame as signal_test observes it on Linux: the restorer at the
    /// handler's rsp (rsp + 8 16-aligned, as after a call), the ucontext
    /// right above, siginfo 304 bytes further, the FPU state 64-aligned
    /// above the frame and below the red zone.
    #[test]
    fn a_handler_frame() {
        let mut regs = UserRegs::default();
        for (i, r) in regs.gpr.iter_mut().enumerate() {
            *r = 0x100 + i as u64;
        }
        regs.gpr[4] = 0x7fff_ffff_e123;
        regs.rip = 0x40_2000;
        regs.rflags = 0x202 | DF;
        let a = handler(0x40_3000, SA_SIGINFO, 0);
        let info = pipe_info();
        let f = build_frame(&regs, SIGPIPE, &info, &a, SigSet::of(SIGUSR1), 832).unwrap();
        assert_eq!((f.addr + 8) % 16, 0);
        assert_eq!(f.fpstate % 64, 0);
        assert!(f.fpstate + 832 <= regs.gpr[4] - RED_ZONE);
        assert!(f.addr + abi::RT_SIGFRAME_SIZE as u64 <= f.fpstate);
        let r = &f.regs;
        assert_eq!((r.rip, r.gpr[4], r.gpr[7]), (0x40_3000, f.addr, 13));
        assert_eq!((r.gpr[2], r.gpr[6]), (f.addr + 8, f.addr + 312));
        assert_eq!((r.gpr[0], r.rflags & DF), (0, 0));
        assert_eq!(r.gpr[3], regs.gpr[3]);
        assert_eq!(f.bytes[..8], 0x40_1000u64.to_le_bytes());
        let uc = Ucontext::from_bytes(f.bytes[8..8 + abi::UCONTEXT_SIZE].try_into().unwrap());
        assert_eq!(regs_of(&uc.uc_mcontext).gpr, regs.gpr);
        assert_eq!(uc.uc_mcontext.rip, regs.rip);
        assert_eq!(uc.uc_mcontext.fpstate, f.fpstate);
        assert_eq!((uc.uc_sigmask, uc.uc_mcontext.oldmask), (1 << 9, 1 << 9));
        assert_eq!(uc.uc_flags, 6);
        assert_eq!(f.bytes[312..440], info.to_bytes());
        // No room below a tiny stack pointer.
        assert!(build_frame(&UserRegs::default(), SIGPIPE, &info, &a, SigSet::EMPTY, 0).is_none());
    }
}
