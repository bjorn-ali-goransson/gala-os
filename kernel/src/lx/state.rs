//! A Linux process's own state beside its core process (`nt::Process`
//! until ROADMAP LX seam 3 splits the core from NT): the program break,
//! the file-descriptor table, pipes, and the line buffers that carry
//! stdout and stderr to the log.
//!
//! Kept in a table keyed by pid (`get`), added by `exec` and dropped when
//! the process ends (`ended`, from `nt::process::finish`).
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

use crate::mm::heap::{AllocError, KArc, TryVec};
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

/// The bytes in a pipe and its open ends.
pub struct PipeState {
    pub data: VecDeque<u8>,
    pub readers: u32,
    pub writers: u32,
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
}

impl Drop for Fd {
    fn drop(&mut self) {
        match &self.object {
            Object::PipeRead(p) => p.state.lock().readers -= 1,
            Object::PipeWrite(p) => p.state.lock().writers -= 1,
            Object::Console(_) => {}
        }
    }
}

/// An open file description (shared by dup'ed fds once dup exists; one
/// per fd for now).
pub struct Fd {
    pub object: Object,
    /// O_NONBLOCK.
    pub nonblock: bool,
}

impl Fd {
    pub fn new(object: Object, nonblock: bool) -> Fd {
        match &object {
            Object::PipeRead(p) => p.state.lock().readers += 1,
            Object::PipeWrite(p) => p.state.lock().writers += 1,
            Object::Console(_) => {}
        }
        Fd { object, nonblock }
    }
}

/// The program break (brk(2)): `start` is fixed at exec, `current` is
/// what brk last set; the pages up to `current` rounded up are mapped.
#[derive(Clone, Copy, Debug)]
pub struct Brk {
    pub start: u64,
    pub current: u64,
}

pub struct LxProcess {
    pub pid: u64,
    pub brk: IrqMutex<Brk>,
    /// Indexed by fd; None: closed.
    pub fds: IrqMutex<Vec<Option<KArc<Fd>>>>,
    /// stdout's and stderr's bytes since their last newline.
    lines: IrqMutex<[Vec<u8>; 2]>,
    /// The unimplemented system calls already logged (once each).
    missing: IrqMutex<Vec<u64>>,
}

static TABLE: IrqMutex<Vec<KArc<LxProcess>>> = IrqMutex::new(Vec::new());

/// Creates the state of a new Linux process `pid` with fds 0, 1 and 2 on
/// the console.
pub fn create(pid: u64, brk: u64) -> Result<KArc<LxProcess>, AllocError> {
    let mut fds = Vec::new();
    for stream in [Stream::Stdin, Stream::Stdout, Stream::Stderr] {
        fds.try_push(Some(KArc::try_new(Fd::new(Object::Console(stream), false))?))?;
    }
    let state = KArc::try_new(LxProcess {
        pid,
        brk: IrqMutex::new(Brk { start: brk, current: brk }),
        fds: IrqMutex::new(fds),
        lines: IrqMutex::new([Vec::new(), Vec::new()]),
        missing: IrqMutex::new(Vec::new()),
    })?;
    TABLE.lock().try_push(state.clone())?;
    Ok(state)
}

/// The state of Linux process `pid`.
pub fn get(pid: u64) -> Option<KArc<LxProcess>> {
    TABLE.lock().iter().find(|p| p.pid == pid).cloned()
}

/// How a Linux process ended, as a shell (and `cargo xtask lx-expect`)
/// reports it: an exit status (exit_group's argument & 0xff), or the
/// signal an exception stands for (an NT exception status ends it today:
/// no signal delivery yet).
pub struct Ending(pub NtStatus);

impl fmt::Display for Ending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.0;
        if s.0 <= 0xff {
            return write!(f, "-- exit status {}", s.0);
        }
        let signal = match s {
            NtStatus::ACCESS_VIOLATION => 11,      // SIGSEGV
            NtStatus::ILLEGAL_INSTRUCTION => 4,    // SIGILL
            NtStatus::INTEGER_DIVIDE_BY_ZERO => 8, // SIGFPE
            NtStatus::BREAKPOINT => 5,             // SIGTRAP
            NtStatus::STACK_OVERFLOW => 11,
            _ => 9, // SIGKILL: ended from outside
        };
        write!(f, "-- killed by signal {signal}")
    }
}

/// Linux process `pid` has ended with `status`: what is left of its
/// output lines is logged, then how it ended; its state goes.
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
    // The fds close with the state (pipe ends counted down).
    let fds = core::mem::take(&mut *state.fds.lock());
    drop(fds);
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
        usize::try_from(fd).ok().and_then(|i| fds.get(i)).and_then(|f| f.clone())
    }

    /// Puts `fd` in the lowest free slot; EMFILE when there is none.
    pub fn install(&self, fd: KArc<Fd>) -> Result<u64, u16> {
        let mut fds = self.fds.lock();
        if let Some(i) = fds.iter().position(Option::is_none) {
            fds[i] = Some(fd);
            return Ok(i as u64);
        }
        if fds.len() >= MAX_FDS {
            return Err(super::errno::EMFILE);
        }
        fds.try_push(Some(fd)).map_err(|_| super::errno::ENOMEM)?;
        Ok(fds.len() as u64 - 1)
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
        state: IrqMutex::new(PipeState { data: VecDeque::new(), readers: 0, writers: 0 }),
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
