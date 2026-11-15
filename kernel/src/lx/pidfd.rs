//! pidfds for the Linux personality: pidfd_open(2), pidfd_send_signal(2)
//! and waitid's P_PIDFD (`fork::waitid`). user/lx/pidfd_test, recorded on
//! Linux 6.18, is the spec. GLib 2.88 (Ubuntu 26.04) watches each child
//! with one: it polls it, then reaps the child with waitid(P_PIDFD).
//!
//! A pidfd refers to a process by its `Life` (shared with the
//! `LxProcess`, not the process itself: holding the process would keep
//! its descriptors open after its end): `ended` is set by `state::ended`,
//! which wakes the pidfds' waiters. A pidfd is readable (POLLIN) once its
//! process has ended (a zombie or reaped), as Linux's.
//!
//! Limits: a pidfd only for a process's main thread (Linux's PIDFD_THREAD
//! pidfds are not made: EINVAL); siginfo given to pidfd_send_signal is
//! not used (the kill's own, ledgered); pidfd_getfd is not done.

use core::sync::atomic::{AtomicBool, Ordering};

use super::dispatch::{Call, Result};
use super::errno::{EBADF, EINVAL, ENOMEM, ESRCH};
use super::state::{Fd, Object};
use crate::mm::heap::KArc;
use crate::nt::ledger;

/// pidfd_open's PIDFD_NONBLOCK (O_NONBLOCK).
pub const PIDFD_NONBLOCK: u64 = 0o4000;
/// pidfd_send_signal's flags (Linux 6.9): the thread, its group, its
/// process group; one at most.
const SIGNAL_THREAD: u64 = 1;
const SIGNAL_THREAD_GROUP: u64 = 2;
const SIGNAL_PROCESS_GROUP: u64 = 4;

/// A process's life as its pidfds see it.
pub struct Life {
    pub ended: AtomicBool,
    pub waiters: KArc<crate::unix::Waiters>,
}

impl Life {
    pub fn new() -> core::result::Result<KArc<Life>, crate::mm::heap::AllocError> {
        let waiters = KArc::try_new(crate::unix::Waiters::default())?;
        KArc::try_new(Life { ended: AtomicBool::new(false), waiters })
    }

    /// The process ended: its pidfds become readable.
    pub fn end(&self) {
        self.ended.store(true, Ordering::Release);
        self.waiters.wake_all();
    }
}

/// An open pidfd: the process's pid and life.
pub struct PidFd {
    pub pid: u64,
    pub life: KArc<Life>,
}

impl PidFd {
    pub fn ended(&self) -> bool {
        self.life.ended.load(Ordering::Acquire)
    }
}

/// The pidfd behind descriptor `fd`: EBADF if it is not open or not one.
pub fn of(call: &Call<'_>, fd: u64) -> core::result::Result<(KArc<Fd>, KArc<PidFd>), u16> {
    let f = call.lx.fd(fd).ok_or(EBADF)?;
    let Object::PidFd(p) = &f.object else { return Err(EBADF) };
    let p = p.clone();
    Ok((f, p))
}

/// pidfd_open(pid, flags): a close-on-exec descriptor for process `pid`
/// (EINVAL for pid 0 or below and flags other than PIDFD_NONBLOCK, ESRCH
/// for no such process).
pub fn pidfd_open(call: &Call<'_>, pid: u64, flags: u64) -> Result {
    if flags & !PIDFD_NONBLOCK != 0 || pid as i32 <= 0 {
        return Err(EINVAL);
    }
    let target = super::state::get(pid).ok_or(ESRCH)?;
    let p = KArc::try_new(PidFd { pid, life: target.life.clone() }).map_err(|_| ENOMEM)?;
    let fd = KArc::try_new(Fd::new(Object::PidFd(p), flags & PIDFD_NONBLOCK != 0))
        .map_err(|_| ENOMEM)?;
    call.lx.install_from(fd, true, 0)
}

/// pidfd_send_signal(pidfd, sig, info, flags): kill(2) of the pidfd's
/// process (EBADF for what is not a pidfd, EINVAL for other flags or more
/// than one; 0 and no effect for a zombie; ESRCH once it is reaped).
pub fn pidfd_send_signal(call: &Call<'_>, fd: u64, sig: u64, info: u64, flags: u64) -> Result {
    let (_f, p) = of(call, fd)?;
    let scope = SIGNAL_THREAD | SIGNAL_THREAD_GROUP | SIGNAL_PROCESS_GROUP;
    if flags & !scope != 0 || (flags & scope).count_ones() > 1 {
        return Err(EINVAL);
    }
    if p.ended() {
        // A zombie (ended, not reaped) takes a signal as Linux's does:
        // nothing happens, 0. Reaped: ESRCH.
        return if super::state::is_zombie(p.pid) { Ok(0) } else { Err(ESRCH) };
    }
    if info != 0 {
        ledger::record("pidfd_send_signal", "linux option missing", info);
    }
    if flags & SIGNAL_PROCESS_GROUP != 0 {
        let pgid =
            super::state::get(p.pid).ok_or(ESRCH)?.pgid.load(core::sync::atomic::Ordering::Relaxed);
        return super::sigcalls::kill_call(call, (pgid as i64).wrapping_neg() as u64, sig);
    }
    // The thread (its main thread: the only pidfds made) or its process.
    super::sigcalls::kill_call(call, p.pid, sig)
}
