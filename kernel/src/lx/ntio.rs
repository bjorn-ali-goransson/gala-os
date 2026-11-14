//! Linux file descriptors on the Windows side's objects (ROADMAP CT, the
//! WSL facade; ARCHITECTURE "The WSL facade"): a distribution's process
//! started by `wsl.exe` (`lx::distro`) gets the standard handles of its
//! starter as fds 0, 1 and 2, as WSL's relay hands a Linux process its
//! caller's console or pipes. One kind of object per handle:
//!
//! - **a pipe end** (an anonymous pipe's or a named pipe's): read and
//!   write through `nt::pipe` (a read waits for bytes; a closed peer is
//!   end of file, or EPIPE and SIGPIPE for a write);
//! - **console input or output**: of the starter's console session (a
//!   console window, a pseudo console), or the serial line without one.
//!   Output goes into the session's ring (and the serial line for a
//!   window's session) as NtWriteFile's does; input is the session's
//!   keys, cooked or raw as its mode says. Such fds are a terminal
//!   (`Terminal`): TCGETS/TCSETS keep a termios whose ICANON, ECHO and
//!   ISIG set the session's ENABLE_LINE_INPUT, ENABLE_ECHO_INPUT and
//!   ENABLE_PROCESSED_INPUT (the console's line editing is the line
//!   discipline: Enter arrives as CR, made NL under ICRNL, a cooked
//!   line's CR LF as one NL); TIOCGWINSZ is the session's size;
//! - **a file** (a redirection to or from a file): read and written at
//!   the handle's position, which moves on;
//! - **NUL**: end of file, writes discarded.
//!
//! Limits (ledgered `linux option missing`, `ntio`): the waits are NT
//! waits, so a Linux signal does not interrupt a read blocked on a pipe
//! or the console (the process's end does); poll and epoll report such
//! fds ready at once (no wait queue: a read then waits); process groups
//! and job control (TIOCGPGRP and friends) are not there (ENOTTY, so a
//! shell turns job control off).

use alloc::vec::Vec;

use lx_abi::termios::{self, Termios, Winsize};

use super::errno::*;
use crate::mm::heap::KArc;
use crate::nt::conhost::{self, Session};
use crate::nt::object::{HandleEntry, ObjectBody, ObjectRef};
use crate::nt::pipe::Outcome;
use crate::nt::status::NtStatus;
use crate::nt::{file, ledger, platform, wait};
use crate::sync::IrqMutex;

/// The most bytes one read or write moves through a kernel buffer.
pub const CHUNK: usize = 64 * 1024;

/// What a handle's object is to a Linux fd.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pipe,
    ConsoleInput,
    ConsoleOutput,
    File,
    Null,
}

/// A terminal: a console session as Linux's tty calls see it, shared by
/// the console fds of one start (and their dups and forks).
pub struct Terminal {
    /// The session; None: the serial line (no keyboard).
    pub session: Option<KArc<Session>>,
    /// What TCGETS reports (a fresh pty's until TCSETS).
    termios: IrqMutex<Termios>,
    /// The CR of a cooked line's CR LF went out as NL: its LF is dropped.
    skip_lf: IrqMutex<bool>,
}

impl Terminal {
    pub fn new(session: Option<KArc<Session>>) -> Result<KArc<Terminal>, NtStatus> {
        KArc::try_new(Terminal {
            session,
            termios: IrqMutex::new(Termios::fresh_pty()),
            skip_lf: IrqMutex::new(false),
        })
        .map_err(|_| NtStatus::NO_MEMORY)
    }
}

/// A Linux fd's Windows-side object.
pub struct NtFile {
    object: ObjectRef,
    pub kind: Kind,
    readable: bool,
    writable: bool,
    /// For console handles.
    terminal: Option<KArc<Terminal>>,
}

const FILE_READ_DATA: u32 = 0x1;
const FILE_WRITE_DATA: u32 = 0x2;
const FILE_APPEND_DATA: u32 = 0x4;

impl NtFile {
    /// The fd for handle `entry` (console handles share `terminal`);
    /// STATUS_NOT_SUPPORTED for another kind of object (ledgered).
    pub fn of(entry: &HandleEntry, terminal: &KArc<Terminal>) -> Result<NtFile, NtStatus> {
        let access = entry.access;
        let (kind, readable, writable) = match &entry.object.body {
            ObjectBody::Pipe(_) => (
                Kind::Pipe,
                access & FILE_READ_DATA != 0,
                access & (FILE_WRITE_DATA | FILE_APPEND_DATA) != 0,
            ),
            ObjectBody::ConsoleInput => (Kind::ConsoleInput, true, false),
            ObjectBody::ConsoleOutput => (Kind::ConsoleOutput, false, true),
            ObjectBody::File(f) if !f.directory => (
                Kind::File,
                access & FILE_READ_DATA != 0,
                access & (FILE_WRITE_DATA | FILE_APPEND_DATA) != 0,
            ),
            ObjectBody::Null(_) => (Kind::Null, true, true),
            _ => {
                ledger::record("ntio", "linux option missing", 0);
                crate::warn!(
                    "lx: a standard handle to a {} is not a Linux fd",
                    entry.object.type_name()
                );
                return Err(NtStatus::NOT_SUPPORTED);
            }
        };
        let terminal =
            matches!(kind, Kind::ConsoleInput | Kind::ConsoleOutput).then(|| terminal.clone());
        Ok(NtFile { object: entry.object.clone(), kind, readable, writable, terminal })
    }

    pub fn readable(&self) -> bool {
        self.readable
    }

    pub fn writable(&self) -> bool {
        self.writable
    }

    /// The fd is a terminal (isatty).
    pub fn is_terminal(&self) -> bool {
        self.terminal.is_some()
    }

    /// Reads up to `out.len()` bytes, waiting for some (0: end of file).
    pub fn read(&self, out: &mut [u8]) -> Result<usize, u16> {
        if !self.readable {
            return Err(EBADF);
        }
        if out.is_empty() {
            return Ok(0);
        }
        match (&self.object.body, self.kind) {
            (ObjectBody::Pipe(end), _) => match end.read(out.len(), None) {
                Ok(Outcome::Done(status, data)) if status.0 < 0x8000_0000 => {
                    let n = data.len().min(out.len());
                    out[..n].copy_from_slice(&data[..n]);
                    Ok(n)
                }
                // The other end closed (or the pipe is gone): end of file.
                Ok(Outcome::Done(..)) => Ok(0),
                Ok(Outcome::Pending) => Err(EAGAIN),
                Err(NtStatus::THREAD_IS_TERMINATING) => Err(EINTR),
                Err(_) => Ok(0),
            },
            (ObjectBody::File(f), _) => {
                let at = *f.position.lock();
                let n = file::read(f.node, at, out).map_err(|_| EIO)?;
                *f.position.lock() = at + n as u64;
                Ok(n)
            }
            (_, Kind::ConsoleInput) => self.console_read(out),
            _ => Ok(0),
        }
    }

    /// Writes `data` (all of it, waiting for room in a pipe); `pid`: the
    /// writer, for the serial line's line discipline.
    pub fn write(&self, pid: u64, data: &[u8]) -> Result<usize, u16> {
        if !self.writable {
            return Err(EBADF);
        }
        match (&self.object.body, self.kind) {
            (ObjectBody::Pipe(end), _) => match end.write(data) {
                Ok(()) => Ok(data.len()),
                Err(NtStatus::NO_MEMORY) => Err(ENOMEM),
                Err(NtStatus::THREAD_IS_TERMINATING) => Err(EINTR),
                Err(_) => Err(EPIPE),
            },
            (ObjectBody::File(f), _) => {
                let at = *f.position.lock();
                let now = platform::system_time();
                let n = file::write(f.node, at, data, now).map_err(|_| EIO)?;
                *f.position.lock() = at + n as u64;
                Ok(n)
            }
            (_, Kind::ConsoleOutput) => {
                let session = self.terminal.as_ref().and_then(|t| t.session.clone());
                // A pseudo console's output goes to its terminal only, as
                // NtWriteFile's does.
                if !session.as_ref().is_some_and(|s| s.pseudo) {
                    crate::console::write_bytes(pid, data);
                }
                if let Some(s) = session
                    && let Some(event) = s.write(data)
                {
                    wait::set_signal(&event, 1);
                }
                Ok(data.len())
            }
            _ => Ok(data.len()),
        }
    }

    /// A read of the console's keys: a cooked line (or the characters
    /// typed so far in raw mode), Enter's CR as NL under ICRNL.
    fn console_read(&self, out: &mut [u8]) -> Result<usize, u16> {
        let Some(term) = &self.terminal else { return Ok(0) };
        let Some(session) = &term.session else { return Ok(0) };
        let mut raw = Vec::new();
        raw.try_reserve_exact(out.len().min(4096)).map_err(|_| ENOMEM)?;
        raw.resize(out.len().min(4096), 0);
        loop {
            if let Some(event) = session.cook() {
                wait::set_signal(&event, 1);
            }
            session.settle();
            let n = session.read_line(&mut raw);
            if n > 0 {
                let icrnl = term.termios.lock().iflag & termios::ICRNL != 0;
                let cooked = session.mode() & conhost::ENABLE_LINE_INPUT != 0;
                let mut skip = term.skip_lf.lock();
                let mut k = 0;
                for &b in &raw[..n] {
                    match b {
                        b'\n' if *skip => *skip = false,
                        b'\r' if icrnl => {
                            out[k] = b'\n';
                            k += 1;
                            *skip = cooked;
                        }
                        b => {
                            out[k] = b;
                            k += 1;
                            *skip = false;
                        }
                    }
                }
                if k > 0 {
                    return Ok(k);
                }
                continue;
            }
            let ready = core::slice::from_ref(&session.input_ready);
            if wait::wait_for(ready, false, wait::Timeout::Infinite)
                == NtStatus::THREAD_IS_TERMINATING
            {
                return Err(EINTR);
            }
        }
    }

    /// A tty ioctl on a console fd: the terminal's termios and size
    /// (see the module's doc); `read`/`write`: the caller's memory.
    pub fn ioctl(
        &self,
        cmd: u32,
        read: impl FnOnce(&mut [u8]) -> Result<(), u16>,
        write: impl FnOnce(&[u8]) -> Result<(), u16>,
    ) -> Result<u64, u16> {
        let Some(term) = &self.terminal else { return Err(ENOTTY) };
        match cmd {
            termios::TCGETS => {
                let t = *term.termios.lock();
                write(&termios_bytes(&t))?;
                Ok(0)
            }
            termios::TCSETS | termios::TCSETSW | termios::TCSETSF => {
                let mut raw = [0u8; 36];
                read(&mut raw)?;
                let t = termios_from(&raw);
                *term.termios.lock() = t;
                if let Some(s) = &term.session {
                    let bit = |on: bool, b: u32| if on { b } else { 0 };
                    let keep = s.mode()
                        & !(conhost::ENABLE_LINE_INPUT
                            | conhost::ENABLE_ECHO_INPUT
                            | conhost::ENABLE_PROCESSED_INPUT);
                    s.set_mode(
                        keep | bit(t.lflag & termios::ICANON != 0, conhost::ENABLE_LINE_INPUT)
                            | bit(t.lflag & termios::ECHO != 0, conhost::ENABLE_ECHO_INPUT)
                            | bit(t.lflag & termios::ISIG != 0, conhost::ENABLE_PROCESSED_INPUT),
                    );
                }
                Ok(0)
            }
            termios::TIOCGWINSZ => {
                let (col, row) = term.session.as_ref().map_or((80, 25), |s| s.size());
                let w = Winsize { row, col, xpixel: 0, ypixel: 0 };
                let mut b = [0u8; 8];
                b[0..2].copy_from_slice(&w.row.to_le_bytes());
                b[2..4].copy_from_slice(&w.col.to_le_bytes());
                write(&b)?;
                Ok(0)
            }
            _ => {
                ledger::record("ntio ioctl", "linux option missing", u64::from(cmd));
                Err(ENOTTY)
            }
        }
    }
}

/// The kernel's struct termios as bytes.
fn termios_bytes(t: &Termios) -> [u8; 36] {
    let mut b = [0u8; 36];
    b[0..4].copy_from_slice(&t.iflag.to_le_bytes());
    b[4..8].copy_from_slice(&t.oflag.to_le_bytes());
    b[8..12].copy_from_slice(&t.cflag.to_le_bytes());
    b[12..16].copy_from_slice(&t.lflag.to_le_bytes());
    b[16] = t.line;
    b[17..].copy_from_slice(&t.cc);
    b
}

fn termios_from(b: &[u8; 36]) -> Termios {
    let word = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let mut cc = [0u8; termios::NCCS];
    cc.copy_from_slice(&b[17..]);
    Termios { iflag: word(0), oflag: word(4), cflag: word(8), lflag: word(12), line: b[16], cc }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn termios_round_trips_as_linux_lays_it_out() {
        let mut t = Termios::fresh_pty();
        t.make_raw();
        let b = termios_bytes(&t);
        assert_eq!(termios_from(&b), t);
        // c_iflag first, c_cc from byte 17 (VINTR first: ^C).
        assert_eq!(b[17], 3);
    }
}
