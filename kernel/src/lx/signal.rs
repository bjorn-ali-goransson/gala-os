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
//!   the thread-directed pending set (SIGPIPE, faults, tgkill) are each
//!   thread's (`ThreadSignals`, by core thread id), the process-directed
//!   pending set (kill, SIGCHLD, SIGALRM) the process's: any thread not
//!   blocking such a signal takes it (`Target`).
//! - A standard signal (1..=31) is pending at most once (a second one is
//!   merged); a real-time one queues each siginfo.
//! - Generation discards a signal whose action ignores it (SIG_IGN, or
//!   SIG_DFL with an Ign default) unless it is blocked: a blocked one
//!   stays pending (signal_test: "blocked and ignored: left pending").
//!   Setting an ignoring action discards what is pending.
//! - Delivery happens on the way back to user mode from a system call
//!   (`lx::dispatch`), the lowest-numbered deliverable signal first, one
//!   handler frame per return: the next one is delivered when its
//!   rt_sigreturn returns. A thread blocked in a wait records itself as
//!   its `sleeper`; raising a signal for it wakes it (`sleepers`).

use alloc::vec::Vec;

use lx_abi::signal::{
    self as abi, DefaultAction, KSigaction, SA_NODEFER, SA_RESETHAND, SIG_DFL, SIG_IGN, SigSet,
    Siginfo,
};

use crate::mm::heap::{AllocError, TryVec};
use crate::nt::platform::ThreadId;
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
    /// The thread, while it blocks in a wait a signal ends (`lx::wait`):
    /// a signal raised for it wakes it.
    pub sleeper: Option<ThreadId>,
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

/// Who a signal is generated for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// One thread (by its core thread id): SIGPIPE, faults, tgkill.
    Thread(u64),
    /// The process: kill, SIGCHLD, SIGALRM; any thread not blocking it
    /// takes it.
    Process,
}

/// A process's signal state.
pub struct Signals {
    /// Signal n's action at n - 1.
    actions: [KSigaction; abi::NSIG as usize],
    /// Process-directed pending signals (kill, SIGCHLD).
    pub process: Pending,
    /// Each thread's mask and pending signals, by core thread id.
    threads: Vec<(u64, ThreadSignals)>,
}

impl Default for Signals {
    fn default() -> Self {
        Signals {
            actions: [KSigaction::default(); abi::NSIG as usize],
            process: Pending::default(),
            threads: Vec::new(),
        }
    }
}

impl Signals {
    /// A forked child's state: the same actions, nothing pending, no
    /// thread yet (fork(2); its thread is added with the forking thread's
    /// mask: `add_thread`).
    pub fn forked(&self) -> Signals {
        Signals { actions: self.actions, process: Pending::default(), threads: Vec::new() }
    }

    /// A new thread `tid` with `mask` (clone(2): the creating thread's).
    pub fn add_thread(&mut self, tid: u64, mask: SigSet) -> Result<(), AllocError> {
        self.threads.try_push((tid, ThreadSignals { mask, ..ThreadSignals::default() }))
    }

    /// Thread `tid` ended: what was pending for it alone goes with it.
    pub fn remove_thread(&mut self, tid: u64) {
        self.threads.retain(|(t, _)| *t != tid);
    }

    /// The threads' ids.
    pub fn thread_ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.threads.iter().map(|(t, _)| *t)
    }

    /// Thread `tid`'s part.
    pub fn thread(&self, tid: u64) -> Option<&ThreadSignals> {
        self.threads.iter().find(|(t, _)| *t == tid).map(|(_, s)| s)
    }

    fn thread_mut(&mut self, tid: u64) -> Option<&mut ThreadSignals> {
        self.threads.iter_mut().find(|(t, _)| *t == tid).map(|(_, s)| s)
    }

    /// Thread `tid`'s blocked mask (empty for a thread not known).
    pub fn mask(&self, tid: u64) -> SigSet {
        self.thread(tid).map_or(SigSet::EMPTY, |t| t.mask)
    }

    /// Records (or clears) the scheduler thread blocked for `tid`.
    pub fn set_sleeper(&mut self, tid: u64, sleeper: Option<ThreadId>) {
        if let Some(t) = self.thread_mut(tid) {
            t.sleeper = sleeper;
        }
    }

    /// The blocked threads a signal for `target` must wake: the thread,
    /// or every thread of the process (one not blocking it takes it; one
    /// in rt_sigtimedwait may wait for it blocked).
    pub fn sleepers(&self, target: Target) -> impl Iterator<Item = ThreadId> + '_ {
        self.threads
            .iter()
            .filter(move |(t, _)| match target {
                Target::Thread(tid) => *t == tid,
                Target::Process => true,
            })
            .filter_map(|(_, s)| s.sleeper)
    }

    /// rt_sigsuspend: `mask` (without SIGKILL/SIGSTOP) until a signal is
    /// delivered; the mask it replaces is kept to put back.
    pub fn suspend(&mut self, tid: u64, mask: SigSet) {
        if let Some(t) = self.thread_mut(tid) {
            let old = core::mem::replace(&mut t.mask, mask.blockable());
            t.saved_mask.get_or_insert(old);
        }
    }

    /// The mask a handler frame saves (rt_sigsuspend's, if it replaced
    /// one: taken), given the one in effect before the handler (`old`).
    pub fn frame_mask(&mut self, tid: u64, old: SigSet) -> SigSet {
        self.thread_mut(tid).and_then(|t| t.saved_mask.take()).unwrap_or(old)
    }

    /// No handler ran after rt_sigsuspend: its mask goes back.
    pub fn restore_saved_mask(&mut self, tid: u64) {
        if let Some(t) = self.thread_mut(tid)
            && let Some(m) = t.saved_mask.take()
        {
            t.mask = m;
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
    /// action that ignores the signal discards what is pending of it, for
    /// the process and every thread.
    pub fn set_action(&mut self, sig: u32, mut new: KSigaction) -> KSigaction {
        new.mask = SigSet(new.mask).blockable().0;
        let old = core::mem::replace(&mut self.actions[sig as usize - 1], new);
        if self.ignored(sig) {
            self.process.discard(sig);
            for (_, t) in self.threads.iter_mut() {
                t.pending.discard(sig);
            }
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

    /// rt_sigprocmask for thread `tid`: the old mask, or None for an
    /// unknown `how`.
    pub fn procmask(&mut self, tid: u64, how: u32, set: SigSet) -> Option<SigSet> {
        let t = self.thread_mut(tid)?;
        let new = t.mask.apply(how, set)?;
        Some(core::mem::replace(&mut t.mask, new))
    }

    /// A signal is generated for `target`. An ignored one is discarded
    /// unless blocked: by the thread it is for, or (process-directed) by
    /// some thread of the process.
    pub fn raise(&mut self, info: Siginfo, target: Target) -> Raised {
        let sig = info.signo as u32;
        if !abi::valid(sig) {
            return Raised::Discarded;
        }
        let blocked = match target {
            Target::Thread(tid) => self.mask(tid).contains(sig),
            Target::Process => self.threads.iter().any(|(_, t)| t.mask.contains(sig)),
        };
        if !blocked && self.ignored(sig) {
            return Raised::Discarded;
        }
        match target {
            Target::Thread(tid) => match self.thread_mut(tid) {
                Some(t) => t.pending.add(info),
                None => Raised::Discarded,
            },
            Target::Process => self.process.add(info),
        }
    }

    /// Everything pending for thread `tid` (its own and the process's).
    pub fn pending(&self, tid: u64) -> SigSet {
        let own = self.thread(tid).map_or(SigSet::EMPTY, |t| t.pending.set);
        own.union(self.process.set)
    }

    /// rt_sigpending: what is pending for `tid` and blocked.
    pub fn pending_blocked(&self, tid: u64) -> SigSet {
        SigSet(self.pending(tid).0 & self.mask(tid).0)
    }

    /// The signal to deliver to `tid` next: the lowest pending one it does
    /// not block.
    pub fn deliverable(&self, tid: u64) -> Option<u32> {
        self.pending(tid).minus(self.mask(tid)).lowest()
    }

    /// Takes the lowest pending signal of `tid` in `set` (rt_sigtimedwait,
    /// delivery), the thread's own first.
    pub fn take_from(&mut self, tid: u64, set: SigSet) -> Option<Siginfo> {
        let sig = SigSet(self.pending(tid).0 & set.0).lowest()?;
        let own = self.thread_mut(tid).and_then(|t| t.pending.take(sig));
        own.or_else(|| self.process.take(sig))
    }

    /// A handler of `sig` is entered on thread `tid`: the mask it runs
    /// with (the current one, its sa_mask, and `sig` itself unless
    /// SA_NODEFER); SA_RESETHAND puts the handler back to SIG_DFL (the
    /// flags, mask and restorer stay: signal_test). Returns the mask to
    /// restore on rt_sigreturn.
    pub fn enter_handler(&mut self, tid: u64, sig: u32, action: &KSigaction) -> SigSet {
        let old = self.mask(tid);
        let mut mask = old.union(SigSet(action.mask));
        if action.flags & SA_NODEFER == 0 {
            mask = mask.with(sig);
        }
        if let Some(t) = self.thread_mut(tid) {
            t.mask = mask.blockable();
        }
        if action.flags & SA_RESETHAND != 0 {
            self.actions[sig as usize - 1].handler = SIG_DFL;
        }
        old
    }

    /// rt_sigreturn puts back the interrupted mask.
    pub fn restore_mask(&mut self, tid: u64, mask: SigSet) {
        if let Some(t) = self.thread_mut(tid) {
            t.mask = mask.blockable();
        }
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
        NtStatus::IN_PAGE_ERROR => abi::SIGBUS,
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

#[cfg(test)]
mod tests {
    use super::*;
    use lx_abi::signal::{SA_RESTORER, SA_SIGINFO, SIGCHLD, SIGKILL, SIGPIPE, SIGUSR1, SIGUSR2};

    fn handler(h: u64, flags: u64, mask: u64) -> KSigaction {
        KSigaction { handler: h, flags: flags | SA_RESTORER, restorer: 0x40_1000, mask }
    }

    const T: Target = Target::Thread(1);

    /// A process with one thread, id 1.
    fn one() -> Signals {
        let mut s = Signals::default();
        s.add_thread(1, SigSet::EMPTY).unwrap();
        s
    }

    fn pipe_info() -> Siginfo {
        Siginfo::from_process(SIGPIPE as i32, abi::SI_USER, 4, 1000)
    }

    /// pipe_test on Linux: a blocked SIGPIPE stays pending and
    /// rt_sigtimedwait takes it (si_code 0); ignored, it is discarded.
    #[test]
    fn sigpipe_blocked_ignored_and_taken() {
        let mut s = one();
        let pipe = SigSet::of(SIGPIPE);
        s.procmask(1, abi::SIG_BLOCK, pipe).unwrap();
        assert_eq!(s.raise(pipe_info(), T), Raised::Pending);
        assert_eq!(s.raise(pipe_info(), T), Raised::Merged);
        assert_eq!(s.pending_blocked(1), pipe);
        assert_eq!(s.deliverable(1), None);
        let info = s.take_from(1, pipe).unwrap();
        assert_eq!((info.signo, info.code), (SIGPIPE as i32, 0));
        assert!(s.pending(1).is_empty());
        s.procmask(1, abi::SIG_SETMASK, SigSet::EMPTY).unwrap();
        s.set_action(SIGPIPE, KSigaction { handler: SIG_IGN, ..Default::default() });
        assert_eq!(s.raise(pipe_info(), T), Raised::Discarded);
        // At SIG_DFL it is delivered and terminates.
        s.set_action(SIGPIPE, KSigaction::default());
        assert_eq!(s.raise(pipe_info(), T), Raised::Pending);
        assert_eq!(s.deliverable(1), Some(SIGPIPE));
        assert_eq!(s.disposition(SIGPIPE), Disposition::Terminate { core: false });
    }

    /// signal_test on Linux: a blocked, ignored signal is left pending;
    /// setting SIG_IGN discards a pending one; SIGCHLD's default ignores.
    #[test]
    fn ignoring() {
        let mut s = one();
        assert_eq!(
            s.raise(Siginfo::child(abi::CLD_EXITED, 5, 0, 0), Target::Process),
            Raised::Discarded
        );
        s.set_action(SIGUSR2, KSigaction { handler: SIG_IGN, ..Default::default() });
        s.procmask(1, abi::SIG_BLOCK, SigSet::of(SIGUSR2)).unwrap();
        let usr2 = Siginfo::from_process(SIGUSR2 as i32, abi::SI_USER, 4, 1000);
        assert_eq!(s.raise(usr2, Target::Process), Raised::Pending);
        assert_eq!(s.pending_blocked(1), SigSet::of(SIGUSR2));
        s.set_action(SIGUSR1, handler(0x1234, 0, 0));
        s.raise(Siginfo::from_process(SIGUSR1 as i32, 0, 4, 1000), Target::Process);
        s.set_action(SIGUSR2, KSigaction { handler: SIG_IGN, ..Default::default() });
        assert_eq!(s.pending(1), SigSet::of(SIGUSR1));
    }

    #[test]
    fn real_time_signals_queue() {
        let mut s = one();
        s.procmask(1, abi::SIG_SETMASK, SigSet::FULL).unwrap();
        for v in [1, 2] {
            s.raise(Siginfo::queued(35, 4, 1000, v), Target::Process);
        }
        assert_eq!(s.take_from(1, SigSet::of(35)).unwrap().get_u64(abi::SI_VALUE_OFFSET), 1);
        assert!(s.pending(1).contains(35));
        assert_eq!(s.take_from(1, SigSet::of(35)).unwrap().get_u64(abi::SI_VALUE_OFFSET), 2);
        assert!(s.pending(1).is_empty());
        assert!(s.take_from(1, SigSet::FULL).is_none());
    }

    /// signal_test on Linux: the handler runs with its signal and sa_mask
    /// blocked (not with SA_NODEFER); SA_RESETHAND leaves SIG_DFL with
    /// the flags; SIGKILL never enters a mask.
    #[test]
    fn entering_a_handler() {
        let mut s = one();
        let a = handler(0x1234, SA_SIGINFO, SigSet::of(abi::SIGTERM).with(SIGKILL).0);
        s.set_action(SIGUSR2, a);
        assert_eq!(s.action(SIGUSR2).mask, SigSet::of(abi::SIGTERM).0);
        let a = s.action(SIGUSR2);
        let old = s.enter_handler(1, SIGUSR2, &a);
        assert_eq!(old, SigSet::EMPTY);
        assert_eq!(s.mask(1), SigSet::of(SIGUSR2).with(abi::SIGTERM));
        s.restore_mask(1, old);
        let a = handler(0x1234, SA_SIGINFO | SA_NODEFER, 0);
        s.enter_handler(1, SIGUSR2, &a);
        assert!(!s.mask(1).contains(SIGUSR2));
        s.restore_mask(1, SigSet::EMPTY);
        let a = handler(0x1234, SA_SIGINFO | SA_RESETHAND, 0);
        s.set_action(SIGUSR2, a);
        s.enter_handler(1, SIGUSR2, &a);
        assert!(s.mask(1).contains(SIGUSR2));
        let after = s.action(SIGUSR2);
        assert_eq!((after.handler, after.flags), (SIG_DFL, 0x8400_0004));
        // A fork keeps actions and mask, not what is pending.
        s.raise(Siginfo::from_process(SIGUSR1 as i32, 0, 4, 1000), Target::Process);
        let mut child = s.forked();
        child.add_thread(1, s.mask(1)).unwrap();
        assert!(child.pending(1).is_empty());
        assert_eq!(child.mask(1), s.mask(1));
        assert_eq!(child.action(SIGUSR2), s.action(SIGUSR2));
    }

    /// fork_test on Linux: after execve a caught signal is at SIG_DFL, an
    /// ignored one still ignored, a blocked one still blocked.
    #[test]
    fn execve_resets_handlers() {
        let mut s = one();
        s.set_action(SIGUSR1, handler(0x1234, SA_SIGINFO, 0));
        s.set_action(SIGUSR2, KSigaction { handler: SIG_IGN, ..Default::default() });
        s.procmask(1, abi::SIG_BLOCK, SigSet::of(abi::SIGTERM)).unwrap();
        s.exec();
        assert_eq!(s.action(SIGUSR1), KSigaction::default());
        assert_eq!(s.action(SIGUSR2).handler, SIG_IGN);
        assert!(s.mask(1).contains(abi::SIGTERM));
    }

    /// signal_test on Linux: rt_sigsuspend's mask is in effect until a
    /// handler runs; the handler's frame saves the mask from before it.
    #[test]
    fn suspend_saves_the_mask_for_the_frame() {
        let mut s = one();
        s.set_action(SIGUSR1, handler(0x1234, 0, 0));
        s.procmask(1, abi::SIG_SETMASK, SigSet::of(SIGUSR1)).unwrap();
        s.suspend(1, SigSet::of(SIGKILL).with(abi::SIGTERM));
        assert_eq!(s.mask(1), SigSet::of(abi::SIGTERM));
        s.raise(Siginfo::from_process(SIGUSR1 as i32, 0, 4, 1000), Target::Process);
        assert_eq!(s.deliverable(1), Some(SIGUSR1));
        let a = s.action(SIGUSR1);
        let before = s.enter_handler(1, SIGUSR1, &a);
        assert_eq!(s.frame_mask(1, before), SigSet::of(SIGUSR1));
        assert_eq!(s.thread(1).unwrap().saved_mask, None);
        // Without a handler the old mask goes back at once.
        s.restore_mask(1, SigSet::of(SIGUSR1));
        s.suspend(1, SigSet::EMPTY);
        s.restore_saved_mask(1);
        assert_eq!(s.mask(1), SigSet::of(SIGUSR1));
    }

    /// thread_test on Linux: a tgkill'd signal is pending for its thread
    /// only; a kill'd one for the process, taken by whichever thread
    /// waits for it; a thread blocked in a wait is woken for either.
    #[test]
    fn signals_per_thread() {
        let mut s = one();
        s.add_thread(2, SigSet::EMPTY).unwrap();
        let both = SigSet::of(SIGUSR1).with(SIGUSR2);
        s.procmask(1, abi::SIG_BLOCK, both).unwrap();
        s.procmask(2, abi::SIG_BLOCK, both).unwrap();
        let tkill = Siginfo::from_process(SIGUSR1 as i32, abi::SI_TKILL, 4, 1000);
        assert_eq!(s.raise(tkill, Target::Thread(2)), Raised::Pending);
        assert_eq!(s.pending_blocked(1), SigSet::EMPTY);
        let kill = Siginfo::from_process(SIGUSR2 as i32, abi::SI_USER, 4, 1000);
        assert_eq!(s.raise(kill, Target::Process), Raised::Pending);
        assert_eq!(s.pending_blocked(1), SigSet::of(SIGUSR2));
        assert_eq!(s.pending_blocked(2), both);
        assert_eq!(s.take_from(2, SigSet::of(SIGUSR2)).unwrap().code, abi::SI_USER);
        assert!(s.pending(1).is_empty());
        s.procmask(2, abi::SIG_UNBLOCK, SigSet::of(SIGUSR1)).unwrap();
        assert_eq!((s.deliverable(1), s.deliverable(2)), (None, Some(SIGUSR1)));
        // Sleepers: the thread a signal is for, or all for the process.
        let (a, b) = (ThreadId::from_raw(7), ThreadId::from_raw(8));
        s.set_sleeper(1, Some(a));
        s.set_sleeper(2, Some(b));
        assert!(s.sleepers(Target::Thread(2)).eq([b]));
        assert!(s.sleepers(Target::Process).eq([a, b]));
        s.set_sleeper(1, None);
        assert!(s.sleepers(Target::Process).eq([b]));
        // A thread's end takes what was pending for it alone.
        s.remove_thread(2);
        assert!(s.thread(2).is_none());
        assert_eq!(s.raise(tkill, Target::Thread(2)), Raised::Discarded);
        // A blocked ignored signal stays pending when some thread blocks
        // it (the process-directed rule).
        s.set_action(SIGUSR2, KSigaction { handler: SIG_IGN, ..Default::default() });
        assert_eq!(s.raise(kill, Target::Process), Raised::Pending);
    }

    #[test]
    fn exit_and_kill_statuses() {
        assert_eq!(killed_by(NtStatus(0)), None);
        assert_eq!(killed_by(NtStatus(255)), None);
        assert_eq!(killed_by(killed_status(SIGPIPE)), Some(SIGPIPE));
        assert_eq!(killed_by(killed_status(64)), Some(64));
        assert_eq!(killed_by(NtStatus::ACCESS_VIOLATION), Some(abi::SIGSEGV));
        assert_eq!(killed_by(NtStatus::IN_PAGE_ERROR), Some(abi::SIGBUS));
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
}
