//! A Linux process's own state beside its core process (`nt::Process`
//! until ROADMAP LX seam 3 splits the core from NT): the program break,
//! the file-descriptor table, pipes, signals (`lx::signal`), its parent
//! and children (fork, wait4), and the line buffers that carry stdout
//! and stderr to the log.
//!
//! Kept in a table keyed by pid (`get`), added by `exec` or `fork` and
//! dropped when the process ends (`ended`, from `nt::process::finish`):
//! its fds close first (pipe ends counted down), then its parent learns
//! of the end (a zombie for wait4, SIGCHLD, its waiters woken), so a
//! parent woken by the end reads EOF from a pipe only the child held.
//!
//! Output: what a Linux program writes to fd 1 or 2 (while they are the
//! console) is logged a line at a time as `lx: pid <pid> stdout: <line>`
//! (or `stderr`), and the end as `lx: pid <pid> -- exit status <n>` (or
//! `-- killed by signal <n>`): the transcript format of `cargo xtask
//! lx-expect`, which the boot scenarios compare with each test's
//! expected.txt.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};

use lx_abi::signal::{SA_NOCLDWAIT, SIG_IGN, SIGCHLD, Siginfo};

use super::file::OpenFile;
use super::signal::{self, Signals};
use crate::fs::ramfs::NodeId;
use crate::mm::heap::{AllocError, KArc, TryVec};
use crate::nt::platform::{self, ThreadId};
use crate::nt::status::NtStatus;
use crate::sync::IrqMutex;

/// A pipe's capacity (F_GETPIPE_SZ; Linux's default, write_exit_test).
pub const PIPE_CAPACITY: usize = 65536;
/// The most file descriptors a process may have open (RLIMIT_NOFILE's
/// usual soft limit).
pub const MAX_FDS: usize = 1024;
/// A line longer than this is logged in pieces.
const MAX_LINE: usize = 4096;

/// The console stream an fd refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Stdin,
    Stdout,
    Stderr,
}

impl Stream {
    fn name(self) -> &'static str {
        match self {
            Stream::Stdin => "stdin",
            Stream::Stdout => "stdout",
            Stream::Stderr => "stderr",
        }
    }
}

/// A page of a pipe's ring (Linux keeps a pipe's bytes in whole pages).
pub const PIPE_PAGE: usize = 4096;
/// The largest pipe an unprivileged process may ask for (the default
/// /proc/sys/fs/pipe-max-size).
pub const PIPE_MAX: usize = 1 << 20;

/// The bytes in a pipe and its open ends.
pub struct PipeState {
    pub data: VecDeque<u8>,
    pub readers: u32,
    pub writers: u32,
    /// Its size in bytes, whole pages (F_GETPIPE_SZ, F_SETPIPE_SZ).
    pub capacity: usize,
    /// How far into its first page the bytes start (read from it already):
    /// that page is not free again until it is read through.
    pub head: usize,
}

impl PipeState {
    /// Bytes a write may add: the whole pages free, plus what is left of
    /// the last page in use (writes fill it before taking a new one).
    pub fn free(&self) -> usize {
        self.capacity.saturating_sub(self.head + self.data.len())
    }

    /// Whether a whole page is free (poll's POLLOUT: poll_test on Linux,
    /// a full pipe is writable again once a page of it was read).
    pub fn page_free(&self) -> bool {
        (self.head + self.data.len()).div_ceil(PIPE_PAGE) * PIPE_PAGE < self.capacity
    }

    /// `n` bytes were read from the front.
    pub fn consumed(&mut self, n: usize) {
        self.data.drain(..n);
        self.head = if self.data.is_empty() { 0 } else { (self.head + n) % PIPE_PAGE };
    }
}

pub struct Pipe {
    pub state: IrqMutex<PipeState>,
}

/// What an fd refers to.
#[derive(Clone)]
pub enum Object {
    Console(Stream),
    PipeRead(KArc<Pipe>),
    PipeWrite(KArc<Pipe>),
    /// A regular file or directory (`lx::file`).
    File(KArc<OpenFile>),
    /// A character device (`/dev/null` and friends: `lx::file`).
    Device(super::file::Device),
    /// A TCP socket (`lx::socket`).
    Socket(KArc<super::socket::Socket>),
}

impl Drop for Fd {
    fn drop(&mut self) {
        match &self.object {
            Object::PipeRead(p) => p.state.lock().readers -= 1,
            Object::PipeWrite(p) => p.state.lock().writers -= 1,
            Object::Console(_) | Object::File(_) | Object::Device(_) | Object::Socket(_) => {}
        }
    }
}

/// An open file description, shared by the fds dup'ed from it (their
/// status flags with it; close-on-exec is each fd's own: `Slot`).
pub struct Fd {
    pub object: Object,
    /// O_NONBLOCK (F_SETFL changes it for every dup).
    pub nonblock: AtomicBool,
}

impl Fd {
    pub fn new(object: Object, nonblock: bool) -> Fd {
        match &object {
            Object::PipeRead(p) => p.state.lock().readers += 1,
            Object::PipeWrite(p) => p.state.lock().writers += 1,
            Object::Console(_) | Object::File(_) | Object::Device(_) | Object::Socket(_) => {}
        }
        Fd { object, nonblock: AtomicBool::new(nonblock) }
    }

    pub fn nonblock(&self) -> bool {
        self.nonblock.load(Ordering::Relaxed)
    }
}

/// An fd: its description and its own close-on-exec flag (FD_CLOEXEC).
pub struct Slot {
    pub fd: KArc<Fd>,
    pub cloexec: bool,
}

/// Whether a vfork's parent may go on, and the parent thread waiting.
#[derive(Default)]
pub struct Vfork {
    pub released: bool,
    pub waiter: Option<ThreadId>,
}

/// The program break (brk(2)): `start` is fixed at exec, `current` is
/// what brk last set; the pages up to `current` rounded up are mapped.
#[derive(Clone, Copy, Debug)]
pub struct Brk {
    pub start: u64,
    pub current: u64,
}

/// A process's children: those running, those ended and not yet waited
/// for (zombies: pid and how they ended), and the threads blocked in
/// wait4.
#[derive(Default)]
pub struct Children {
    pub running: Vec<u64>,
    pub ended: Vec<(u64, NtStatus)>,
    pub waiters: Vec<ThreadId>,
}

pub struct LxProcess {
    pub pid: u64,
    /// The parent's pid; 0 for a process the kernel started (`lxrun=`).
    pub parent: u64,
    /// The signal the parent gets when this process ends (SIGCHLD for a
    /// fork; clone's low byte).
    pub exit_signal: u32,
    pub brk: IrqMutex<Brk>,
    pub signals: IrqMutex<Signals>,
    pub children: IrqMutex<Children>,
    /// Indexed by fd; None: closed.
    pub fds: IrqMutex<Vec<Option<Slot>>>,
    /// The current directory (chdir; a forked child's starts as its
    /// parent's).
    pub cwd: IrqMutex<NodeId>,
    /// The file-mode creation mask (umask(2); 022 at the start).
    pub umask: IrqMutex<u32>,
    /// ITIMER_REAL (`lx::timer`; alarm, setitimer).
    pub alarm: IrqMutex<Option<super::timer::Alarm>>,
    /// A vfork child's hold on its parent (`lx::fork`): released when it
    /// execs or ends.
    pub vfork: IrqMutex<Vfork>,
    /// stdout's and stderr's bytes since their last newline.
    lines: IrqMutex<[Vec<u8>; 2]>,
    /// The unimplemented system calls already logged (once each).
    missing: IrqMutex<Vec<u64>>,
}

static TABLE: IrqMutex<Vec<KArc<LxProcess>>> = IrqMutex::new(Vec::new());

/// Creates the state of a new Linux process `pid` with fds 0, 1 and 2 on
/// the console, in directory `cwd`.
pub fn create(pid: u64, brk: u64, cwd: NodeId) -> Result<KArc<LxProcess>, AllocError> {
    let mut fds = Vec::new();
    for stream in [Stream::Stdin, Stream::Stdout, Stream::Stderr] {
        let fd = KArc::try_new(Fd::new(Object::Console(stream), false))?;
        fds.try_push(Some(Slot { fd, cloexec: false }))?;
    }
    let state = KArc::try_new(LxProcess {
        pid,
        parent: 0,
        exit_signal: SIGCHLD,
        brk: IrqMutex::new(Brk { start: brk, current: brk }),
        signals: IrqMutex::new(Signals::default()),
        children: IrqMutex::new(Children::default()),
        fds: IrqMutex::new(fds),
        cwd: IrqMutex::new(cwd),
        umask: IrqMutex::new(0o022),
        vfork: IrqMutex::new(Vfork { released: true, waiter: None }),
        alarm: IrqMutex::new(None),
        lines: IrqMutex::new([Vec::new(), Vec::new()]),
        missing: IrqMutex::new(Vec::new()),
    })?;
    TABLE.lock().try_push(state.clone())?;
    Ok(state)
}

/// The state of a forked child `pid` of `parent`: the same break, the
/// same open file descriptions in the same slots (shared: a pipe end
/// stays open while either holds it), the same signal actions and mask,
/// nothing pending, no children. It is listed, and counted as a running
/// child of `parent`, before its thread starts.
pub fn fork(parent: &LxProcess, pid: u64, vfork: bool) -> Result<KArc<LxProcess>, AllocError> {
    let mut fds = Vec::new();
    {
        let from = parent.fds.lock();
        fds.try_reserve_exact(from.len()).map_err(|_| AllocError)?;
        for slot in from.iter() {
            fds.try_push(slot.as_ref().map(|s| Slot { fd: s.fd.clone(), cloexec: s.cloexec }))?;
        }
    }
    let brk = *parent.brk.lock();
    let signals = parent.signals.lock().forked();
    let cwd = *parent.cwd.lock();
    let umask = *parent.umask.lock();
    let state = KArc::try_new(LxProcess {
        pid,
        parent: parent.pid,
        exit_signal: SIGCHLD,
        brk: IrqMutex::new(brk),
        signals: IrqMutex::new(signals),
        children: IrqMutex::new(Children::default()),
        fds: IrqMutex::new(fds),
        cwd: IrqMutex::new(cwd),
        umask: IrqMutex::new(umask),
        vfork: IrqMutex::new(Vfork { released: !vfork, waiter: None }),
        // A child starts with no timer (setitimer(2)).
        alarm: IrqMutex::new(None),
        lines: IrqMutex::new([Vec::new(), Vec::new()]),
        missing: IrqMutex::new(Vec::new()),
    })?;
    parent.children.lock().running.try_push(pid)?;
    if let Err(e) = TABLE.lock().try_push(state.clone()) {
        parent.children.lock().running.retain(|&c| c != pid);
        return Err(e);
    }
    Ok(state)
}

/// Undoes `fork` for a child whose thread could not start: unlisted, no
/// longer the parent's child, its fds closed; nobody is told.
pub fn discard(child: &LxProcess) {
    TABLE.lock().retain(|p| p.pid != child.pid);
    if let Some(parent) = get(child.parent) {
        parent.children.lock().running.retain(|&c| c != child.pid);
    }
    let fds = core::mem::take(&mut *child.fds.lock());
    drop(fds);
}

/// The state of Linux process `pid`.
pub fn get(pid: u64) -> Option<KArc<LxProcess>> {
    TABLE.lock().iter().find(|p| p.pid == pid).cloned()
}

/// How a Linux process ended, as a shell (and `cargo xtask lx-expect`)
/// reports it: an exit status (exit_group's argument & 0xff), or the
/// signal that killed it (a signal's default action,
/// `signal::killed_status`, or the signal a CPU exception stands for: an
/// NT exception status ends it while faults are not yet signals).
pub struct Ending(pub NtStatus);

impl fmt::Display for Ending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match signal::killed_by(self.0) {
            Some(sig) => write!(f, "-- killed by signal {sig}"),
            None => write!(f, "-- exit status {}", self.0.0),
        }
    }
}

/// Linux process `pid` has ended with `status`: what is left of its
/// output lines is logged, then how it ended; its state goes, its fds
/// close, and then its parent is told (`child_ended`).
pub fn ended(pid: u64, status: NtStatus) {
    let state = {
        let mut table = TABLE.lock();
        let i = table.iter().position(|p| p.pid == pid);
        i.map(|i| table.remove(i))
    };
    let Some(state) = state else { return };
    {
        let mut lines = state.lines.lock();
        for (i, stream) in [Stream::Stdout, Stream::Stderr].into_iter().enumerate() {
            if !lines[i].is_empty() {
                log_line(pid, stream, &lines[i]);
                lines[i].clear();
            }
        }
    }
    crate::info!("lx: pid {pid} {}", Ending(status));
    state.release_vfork();
    // The fds close with the state (pipe ends counted down), before the
    // parent hears of it.
    let fds = core::mem::take(&mut *state.fds.lock());
    drop(fds);
    // Its own children are nobody's now (no reaper yet: they are not
    // waited for).
    state.children.lock().ended.clear();
    if state.parent != 0
        && let Some(parent) = get(state.parent)
    {
        child_ended(&parent, &state, status);
    }
}

/// Child `child` of `parent` ended with `status`: it becomes a zombie
/// for wait4 (unless the parent ignores SIGCHLD or set SA_NOCLDWAIT:
/// then it is reaped at once, wait(2)), the parent gets its exit signal
/// (SIGCHLD: discarded at its default action), and threads waiting in
/// wait4 wake.
fn child_ended(parent: &LxProcess, child: &LxProcess, status: NtStatus) {
    let reap_now = {
        let sigs = parent.signals.lock();
        let chld = sigs.action(SIGCHLD);
        chld.handler == SIG_IGN || chld.flags & SA_NOCLDWAIT != 0
    };
    let waiters = {
        let mut c = parent.children.lock();
        c.running.retain(|&p| p != child.pid);
        if !reap_now && c.ended.try_push((child.pid, status)).is_err() {
            crate::warn!(
                "lx: pid {}: no memory to keep child {}'s status: wait4 will not see it",
                parent.pid,
                child.pid
            );
        }
        core::mem::take(&mut c.waiters)
    };
    let (code, value) = signal::child_code(status);
    let info = if child.exit_signal == SIGCHLD {
        Siginfo::child(code, child.pid as i32, super::exec::UID as u32, value)
    } else {
        Siginfo::from_process(child.exit_signal as i32, 0, child.pid as i32, 0)
    };
    let raised = parent.signals.lock().raise(info, false);
    crate::events::emit(
        crate::events::Category::Process,
        "lx child ended",
        format_args!(
            r#""pid":{},"child":{},"wait_status":{},"signal":{},"raised":"{raised:?}","reaped":{reap_now}"#,
            parent.pid,
            child.pid,
            signal::wait_status(status),
            child.exit_signal
        ),
    );
    for w in waiters {
        platform::wake(w);
    }
}

fn log_line(pid: u64, stream: Stream, line: &[u8]) {
    crate::info!("lx: pid {pid} {}: {}", stream.name(), Lossy(line));
}

impl LxProcess {
    /// Bytes written to the console stream `stream` (stdout or stderr):
    /// logged a line at a time.
    pub fn console_write(&self, stream: Stream, data: &[u8]) -> Result<(), AllocError> {
        let i = if stream == Stream::Stderr { 1 } else { 0 };
        let mut lines = self.lines.lock();
        let buf = &mut lines[i];
        for &b in data {
            if b == b'\n' {
                log_line(self.pid, stream, buf);
                buf.clear();
                continue;
            }
            if buf.len() >= MAX_LINE {
                log_line(self.pid, stream, buf);
                buf.clear();
            }
            buf.try_push(b)?;
        }
        Ok(())
    }

    /// Whether unimplemented system call `number` is new to this process
    /// (it is then remembered; logged once each).
    pub fn first_miss(&self, number: u64) -> bool {
        let mut seen = self.missing.lock();
        !seen.contains(&number) && seen.try_push(number).is_ok()
    }

    /// The open file description of `fd`.
    pub fn fd(&self, fd: u64) -> Option<KArc<Fd>> {
        let fds = self.fds.lock();
        usize::try_from(fd)
            .ok()
            .and_then(|i| fds.get(i))
            .and_then(|s| s.as_ref())
            .map(|s| s.fd.clone())
    }

    /// `fd`'s close-on-exec flag; None if it is not open.
    pub fn cloexec(&self, fd: u64) -> Option<bool> {
        let fds = self.fds.lock();
        usize::try_from(fd)
            .ok()
            .and_then(|i| fds.get(i))
            .and_then(|s| s.as_ref())
            .map(|s| s.cloexec)
    }

    /// Sets `fd`'s close-on-exec flag; false if it is not open.
    pub fn set_cloexec(&self, fd: u64, on: bool) -> bool {
        let mut fds = self.fds.lock();
        match usize::try_from(fd).ok().and_then(|i| fds.get_mut(i)).and_then(|s| s.as_mut()) {
            Some(s) => {
                s.cloexec = on;
                true
            }
            None => false,
        }
    }

    /// Puts `fd` in the lowest free slot; EMFILE when there is none.
    pub fn install(&self, fd: KArc<Fd>) -> Result<u64, u16> {
        self.install_from(fd, false, 0)
    }

    /// Puts `fd` in the lowest free slot at or above `min` (F_DUPFD);
    /// EMFILE when there is none, EINVAL for `min` beyond the limit.
    pub fn install_from(&self, fd: KArc<Fd>, cloexec: bool, min: u64) -> Result<u64, u16> {
        let min = usize::try_from(min).ok().filter(|&m| m < MAX_FDS).ok_or(super::errno::EINVAL)?;
        let mut fds = self.fds.lock();
        let slot = Some(Slot { fd, cloexec });
        if let Some(i) = (min..fds.len()).find(|&i| fds[i].is_none()) {
            fds[i] = slot;
            return Ok(i as u64);
        }
        let at = fds.len().max(min);
        if at >= MAX_FDS {
            return Err(super::errno::EMFILE);
        }
        while fds.len() < at {
            fds.try_push(None).map_err(|_| super::errno::ENOMEM)?;
        }
        fds.try_push(slot).map_err(|_| super::errno::ENOMEM)?;
        Ok(at as u64)
    }

    /// Puts `fd` at slot `target` (dup2, dup3), closing what was there;
    /// EBADF for a slot beyond the limit.
    pub fn install_at(&self, target: u64, fd: KArc<Fd>, cloexec: bool) -> Result<(), u16> {
        let at =
            usize::try_from(target).ok().filter(|&t| t < MAX_FDS).ok_or(super::errno::EBADF)?;
        let old = {
            let mut fds = self.fds.lock();
            while fds.len() <= at {
                fds.try_push(None).map_err(|_| super::errno::ENOMEM)?;
            }
            fds[at].replace(Slot { fd, cloexec })
        };
        // Dropped outside the table's lock (a pipe end counts down).
        drop(old);
        Ok(())
    }

    /// A vfork child execs or ends: its parent goes on.
    pub fn release_vfork(&self) {
        let waiter = {
            let mut v = self.vfork.lock();
            v.released = true;
            v.waiter.take()
        };
        if let Some(w) = waiter {
            platform::wake(w);
        }
    }

    /// execve: the fds marked close-on-exec close.
    pub fn close_on_exec(&self) {
        let mut closed: Vec<Slot> = Vec::new();
        {
            let mut fds = self.fds.lock();
            for slot in fds.iter_mut() {
                if slot.as_ref().is_some_and(|s| s.cloexec)
                    && let Some(s) = slot.take()
                {
                    // Kept to drop outside the lock; with no room it drops
                    // here (closing takes a pipe's or the file system's
                    // lock, never the table's).
                    let _ = closed.try_push(s);
                }
            }
        }
        drop(closed);
    }

    /// Closes `fd`; false if it was not open.
    pub fn close(&self, fd: u64) -> bool {
        let taken = {
            let mut fds = self.fds.lock();
            usize::try_from(fd).ok().and_then(|i| fds.get_mut(i)).and_then(Option::take)
        };
        // Dropped outside the table's lock (a pipe end counts down).
        taken.is_some()
    }
}

/// Text that may not be UTF-8, shown with U+FFFD for bad bytes.
struct Lossy<'a>(&'a [u8]);

impl fmt::Display for Lossy<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for chunk in self.0.utf8_chunks() {
            f.write_str(chunk.valid())?;
            if !chunk.invalid().is_empty() {
                f.write_str("\u{fffd}")?;
            }
        }
        Ok(())
    }
}

/// A new pipe (no ends yet: `Fd::new` counts them).
pub fn new_pipe() -> Result<KArc<Pipe>, AllocError> {
    KArc::try_new(Pipe {
        state: IrqMutex::new(PipeState {
            data: VecDeque::new(),
            readers: 0,
            writers: 0,
            capacity: PIPE_CAPACITY,
            head: 0,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endings_read_as_a_shell_reports_them() {
        assert_eq!(alloc::format!("{}", Ending(NtStatus(44))), "-- exit status 44");
        assert_eq!(alloc::format!("{}", Ending(NtStatus(0))), "-- exit status 0");
        let segv = alloc::format!("{}", Ending(NtStatus::ACCESS_VIOLATION));
        assert_eq!(segv, "-- killed by signal 11");
        // pipe_test: a child at SIGPIPE's default action.
        let pipe = alloc::format!("{}", Ending(signal::killed_status(13)));
        assert_eq!(pipe, "-- killed by signal 13");
    }

    #[test]
    fn a_pipe_frees_whole_pages() {
        // pipe_test on Linux: a full pipe that gave up 100 bytes takes no
        // 100-byte write (its first page is still in use); once 4096 more
        // are read, one page is free.
        let pipe = new_pipe().unwrap();
        let mut st = pipe.state.lock();
        st.data.extend(core::iter::repeat_n(0u8, PIPE_CAPACITY));
        assert_eq!(st.free(), 0);
        st.consumed(100);
        assert_eq!(st.free(), 0);
        st.consumed(4096);
        assert_eq!(st.free(), 4096);
        // Read empty, the next write starts on a fresh page.
        let n = st.data.len();
        st.consumed(n);
        assert_eq!((st.head, st.free()), (0, PIPE_CAPACITY));
    }

    #[test]
    fn pipe_ends_are_counted() {
        let pipe = new_pipe().unwrap();
        let r = Fd::new(Object::PipeRead(pipe.clone()), false);
        let w = Fd::new(Object::PipeWrite(pipe.clone()), true);
        let ends = |p: &Pipe| {
            let st = p.state.lock();
            (st.readers, st.writers)
        };
        assert_eq!(ends(&pipe), (1, 1));
        drop(w);
        assert_eq!(pipe.state.lock().writers, 0);
        drop(r);
        assert_eq!(pipe.state.lock().readers, 0);
    }
}
