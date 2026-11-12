//! Pseudo-terminals for the Linux personality (ROADMAP LD step 5: xterm
//! runs its shell on one; CT step 2's terminals; user/lx/pty_test,
//! recorded on Linux, is the spec): `/dev/ptmx`, `/dev/pts/<n>`,
//! `/dev/tty`, the line discipline and the terminal ioctls. Sources:
//! termios(3), tty_ioctl(4), pts(4), credentials(7) (man-pages), POSIX's
//! "General Terminal Interface", and pty_test's transcript.
//!
//! - **A pty** (`Pty`) is a pair of ends behind descriptors
//!   (`Object::Pty`): opening `/dev/ptmx` makes one (the lowest free
//!   number, locked), TIOCGPTN names it, TIOCSPTLCK unlocks it, and
//!   `/dev/pts/<n>` (or TIOCGPTPEER on the master) opens its slave. Its
//!   number is free again once every master description is closed (the
//!   name goes with it, as devpts' does).
//! - **The line discipline** (`Ldisc`, pure: host tests drive it): what
//!   the master writes is the terminal's input, processed by c_iflag
//!   (ISTRIP, IGNCR, ICRNL, INLCR) and c_lflag: in canonical mode
//!   (ICANON) lines are edited (VERASE, VKILL, VWERASE with IEXTEN,
//!   VLNEXT), ended by NL, VEOL, VEOL2 or VEOF, and a read gets at most
//!   one line (VEOF on an empty line: a read of 0); otherwise bytes are
//!   readable at once (VMIN and VTIME: VMIN 0 returns what is there, or
//!   0 after VTIME tenths of a second; VMIN above 0 waits for a byte).
//!   ISIG's VINTR, VQUIT and VSUSP raise SIGINT, SIGQUIT and SIGTSTP in
//!   the foreground process group and flush the input (unless NOFLSH);
//!   echo (ECHO, ECHOE, ECHOK, ECHOKE, ECHOCTL as `^X`, ECHONL) and what
//!   the slave writes go through c_oflag (OPOST: ONLCR, OCRNL) to the
//!   master.
//! - **Sessions** (`lx::session`): a session leader opening the slave
//!   without O_NOCTTY (or TIOCSCTTY) makes it its controlling terminal;
//!   TIOCGPGRP/TIOCSPGRP the foreground group, TIOCGSID the session,
//!   TIOCSWINSZ a change of size raises SIGWINCH in the foreground group;
//!   SIGTSTP for an orphaned group is discarded (POSIX), and stopping is
//!   not implemented otherwise (ledgered: the group is not stopped).
//! - **Hang-up**: once every slave description is closed, the master's
//!   read fails with EIO; once every master is closed, the slave reads
//!   end of file and its writes fail with EIO, and the session's
//!   foreground group gets SIGHUP.
//! - **Waits**: readers and writers of both ends wait on the pty's
//!   `Waiters` (`poll::Watch` puts poll, select and epoll there too);
//!   every change wakes them all.
//!
//! Limits: IXON's VSTOP and VSTART are taken out of the input but output
//! is never stopped (ledgered); VREPRINT and VDISCARD are ordinary
//! characters; no packet mode (TIOCPKT), no break.
//!
//! Observability: `lx: pid P fd N: pty <n> master|slave (...)` lines in
//! the state dump (queued bytes, mode, session and foreground group),
//! the ledger (`pty ioctl` with the request, `pty` for what is not kept),
//! signals raised by the terminal as `lx signal` events.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use lx_abi::termios::{self, Termios, Winsize};

use super::dispatch::{Call, Result};
use super::errno::{EAGAIN, EFAULT, EINVAL, EIO, ENOMEM, ENOTTY, ENXIO, EPERM};
use super::state::{Fd, Object};
use crate::mm::heap::{KArc, TryVec};
use crate::nt::{ledger, platform};
use crate::sync::IrqMutex;
use crate::unix::Waiters;

/// The longest canonical line kept (Linux's N_TTY_BUF_SIZE less one).
const MAX_LINE: usize = 4095;
/// Bytes queued for either side before more are refused (input) or a
/// writer waits (output).
const MAX_QUEUE: usize = 64 * 1024;
/// ptys at most (devpts' default max is larger; a bound for the table).
const MAX_PTYS: usize = 256;

pub const TIOCSTI: u32 = 0x5412;
pub const TCSBRK: u32 = 0x5409;
pub const TCXONC: u32 = 0x540a;
pub const TCFLSH: u32 = 0x540b;
pub const TIOCOUTQ: u32 = 0x5411;
pub const TIOCGPTPEER: u32 = 0x5441;
pub const FIONBIO: u32 = 0x5421;

const SIGHUP: u32 = 1;
const SIGINT: u32 = 2;
const SIGQUIT: u32 = 3;
const SIGTSTP: u32 = 20;
const SIGWINCH: u32 = 28;

/// The line discipline's state (see the module's doc).
pub struct Ldisc {
    pub t: Termios,
    pub ws: Winsize,
    /// The canonical line being edited.
    line: Vec<u8>,
    /// Lines ended and not read (canonical; an empty one is an end of
    /// file).
    lines: VecDeque<Vec<u8>>,
    /// Bytes readable (non-canonical).
    raw: VecDeque<u8>,
    /// What the master reads: echo and the slave's output.
    pub out: VecDeque<u8>,
    /// The next character is literal (VLNEXT).
    literal: bool,
}

/// What a typed character asks for beyond the queues.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Typed {
    Nothing,
    /// A signal for the foreground process group.
    Signal(u32),
    /// IXON's VSTOP or VSTART (taken out, not acted on).
    FlowControl,
}

fn is_ctrl(c: u8) -> bool {
    c < 0x20 || c == 0x7f
}

impl Ldisc {
    pub fn new() -> Ldisc {
        Ldisc {
            t: Termios::fresh_pty(),
            ws: Winsize::default(),
            line: Vec::new(),
            lines: VecDeque::new(),
            raw: VecDeque::new(),
            out: VecDeque::new(),
            literal: false,
        }
    }

    /// Output processing of `c` into the master's queue.
    fn put(&mut self, c: u8) -> core::result::Result<(), u16> {
        self.out.try_reserve(2).map_err(|_| ENOMEM)?;
        let opost = self.t.oflag & termios::OPOST != 0;
        if opost && c == b'\n' && self.t.oflag & termios::ONLCR != 0 {
            self.out.push_back(b'\r');
            self.out.push_back(b'\n');
        } else if opost && c == b'\r' && self.t.oflag & termios::OCRNL != 0 {
            self.out.push_back(b'\n');
        } else {
            self.out.push_back(c);
        }
        Ok(())
    }

    /// What the slave writes.
    pub fn write(&mut self, data: &[u8]) -> core::result::Result<(), u16> {
        for &c in data {
            self.put(c)?;
        }
        Ok(())
    }

    /// The width echo gave `c` (2 for `^X`).
    fn width(&self, c: u8) -> usize {
        if self.t.lflag & termios::ECHOCTL != 0 && is_ctrl(c) && c != b'\t' && c != b'\n' {
            2
        } else {
            1
        }
    }

    fn echo(&mut self, c: u8) -> core::result::Result<(), u16> {
        if self.t.lflag & termios::ECHO == 0 {
            return Ok(());
        }
        if self.width(c) == 2 {
            self.put(b'^')?;
            self.put(if c == 0x7f { b'?' } else { c + 0x40 })
        } else {
            self.put(c)
        }
    }

    /// Erases the last character of the line (with ECHOE, from the screen
    /// too). False if the line was empty.
    fn erase_one(&mut self) -> core::result::Result<bool, u16> {
        let Some(c) = self.line.pop() else { return Ok(false) };
        if self.t.lflag & termios::ECHO != 0 && self.t.lflag & termios::ECHOE != 0 {
            for _ in 0..self.width(c) {
                self.put(8)?;
                self.put(b' ')?;
                self.put(8)?;
            }
        }
        Ok(true)
    }

    fn end_line(&mut self) -> core::result::Result<(), u16> {
        self.lines.try_reserve(1).map_err(|_| ENOMEM)?;
        let line = core::mem::take(&mut self.line);
        self.lines.push_back(line);
        Ok(())
    }

    /// Drops what was typed and not read.
    pub fn flush_input(&mut self) {
        self.line.clear();
        self.lines.clear();
        self.raw.clear();
        self.literal = false;
    }

    /// A character the master wrote (the terminal's input).
    pub fn type_char(&mut self, mut c: u8) -> core::result::Result<Typed, u16> {
        let (i, l, cc) = (self.t.iflag, self.t.lflag, self.t.cc);
        if i & termios::ISTRIP != 0 {
            c &= 0x7f;
        }
        let literal = core::mem::take(&mut self.literal);
        if !literal {
            if c == b'\r' {
                if i & termios::IGNCR != 0 {
                    return Ok(Typed::Nothing);
                }
                if i & termios::ICRNL != 0 {
                    c = b'\n';
                }
            } else if c == b'\n' && i & termios::INLCR != 0 {
                c = b'\r';
            }
            let is = |v: usize| cc[v] != 0 && c == cc[v];
            if l & termios::ISIG != 0 {
                let sig = if is(termios::VINTR) {
                    Some(SIGINT)
                } else if is(termios::VQUIT) {
                    Some(SIGQUIT)
                } else if is(termios::VSUSP) {
                    Some(SIGTSTP)
                } else {
                    None
                };
                if let Some(sig) = sig {
                    if l & termios::NOFLSH == 0 {
                        self.flush_input();
                    }
                    self.echo(c)?;
                    return Ok(Typed::Signal(sig));
                }
            }
            if i & termios::IXON != 0 && (is(termios::VSTOP) || is(termios::VSTART)) {
                return Ok(Typed::FlowControl);
            }
            if l & termios::IEXTEN != 0 && is(termios::VLNEXT) {
                self.literal = true;
                if l & termios::ECHO != 0 && l & termios::ECHOCTL != 0 {
                    self.put(b'^')?;
                    self.put(8)?;
                }
                return Ok(Typed::Nothing);
            }
            if l & termios::ICANON != 0 {
                if is(termios::VERASE) {
                    self.erase_one()?;
                    return Ok(Typed::Nothing);
                }
                if is(termios::VKILL) {
                    if l & termios::ECHOKE != 0 && l & termios::ECHOE != 0 {
                        while self.erase_one()? {}
                    } else {
                        self.line.clear();
                        if l & termios::ECHO != 0 && l & termios::ECHOK != 0 {
                            self.echo(c)?;
                            self.put(b'\n')?;
                        }
                    }
                    return Ok(Typed::Nothing);
                }
                if l & termios::IEXTEN != 0 && is(termios::VWERASE) {
                    while self.line.last().is_some_and(|&b| b == b' ' || b == b'\t') {
                        self.erase_one()?;
                    }
                    while self.line.last().is_some_and(|&b| b != b' ' && b != b'\t') {
                        self.erase_one()?;
                    }
                    return Ok(Typed::Nothing);
                }
                if is(termios::VEOF) {
                    self.end_line()?;
                    return Ok(Typed::Nothing);
                }
                if c == b'\n' || is(termios::VEOL) || is(termios::VEOL2) {
                    self.line.try_push(c).map_err(|_| ENOMEM)?;
                    if l & termios::ECHO != 0 || (c == b'\n' && l & termios::ECHONL != 0) {
                        self.put(c)?;
                    }
                    self.end_line()?;
                    return Ok(Typed::Nothing);
                }
            }
        }
        if l & termios::ICANON != 0 {
            // The line is full: the character is dropped (as N_TTY does).
            if self.line.len() >= MAX_LINE {
                return Ok(Typed::Nothing);
            }
            self.line.try_push(c).map_err(|_| ENOMEM)?;
        } else {
            if self.raw.len() >= MAX_QUEUE {
                return Ok(Typed::Nothing);
            }
            self.raw.try_reserve(1).map_err(|_| ENOMEM)?;
            self.raw.push_back(c);
        }
        self.echo(c)?;
        Ok(Typed::Nothing)
    }

    /// Bytes a slave read would get now (FIONREAD).
    pub fn readable(&self) -> usize {
        if self.t.canonical() { self.lines.iter().map(Vec::len).sum() } else { self.raw.len() }
    }

    /// Whether a slave read returns at once (data, or an end of file).
    pub fn has_input(&self) -> bool {
        if self.t.canonical() { !self.lines.is_empty() } else { !self.raw.is_empty() }
    }

    /// A slave read of at most `out.len()` bytes: None if nothing is
    /// there (an end-of-file line reads 0).
    pub fn take(&mut self, out: &mut [u8]) -> Option<usize> {
        if self.t.canonical() {
            let line = self.lines.front_mut()?;
            let n = line.len().min(out.len());
            out[..n].copy_from_slice(&line[..n]);
            line.drain(..n);
            if line.is_empty() {
                self.lines.pop_front();
            }
            Some(n)
        } else {
            if self.raw.is_empty() {
                return None;
            }
            let n = self.raw.len().min(out.len());
            for (o, b) in out.iter_mut().zip(self.raw.drain(..n)) {
                *o = b;
            }
            Some(n)
        }
    }

    /// New settings: from canonical to not, what was typed becomes
    /// readable bytes (and back: they stay readable as one line).
    pub fn set(&mut self, t: Termios, flush: bool) {
        if flush {
            self.flush_input();
        }
        let (was, now) = (self.t.canonical(), t.canonical());
        self.t = t;
        if was && !now {
            let mut bytes = Vec::new();
            for l in core::mem::take(&mut self.lines) {
                let _ = bytes.try_extend_from_slice(&l);
            }
            let _ = bytes.try_extend_from_slice(&core::mem::take(&mut self.line));
            if self.raw.try_reserve(bytes.len()).is_ok() {
                self.raw.extend(bytes);
            }
        } else if !was && now && !self.raw.is_empty() {
            let mut bytes = Vec::new();
            if bytes.try_reserve_exact(self.raw.len()).is_ok() && self.lines.try_reserve(1).is_ok()
            {
                bytes.extend(self.raw.drain(..));
                self.lines.push_back(bytes);
            }
        }
    }
}

impl Default for Ldisc {
    fn default() -> Ldisc {
        Ldisc::new()
    }
}

/// A pty's shared state.
pub struct State {
    pub ld: Ldisc,
    pub locked: bool,
    /// Open master and slave descriptions.
    pub masters: u32,
    pub slaves: u32,
    /// The session it is the controlling terminal of (0: none) and that
    /// session's foreground process group.
    pub session: u64,
    pub pgrp: u64,
    /// Changes so far (epoll's edge-triggered reports).
    pub changes: u64,
}

pub struct Pty {
    pub number: u32,
    pub state: IrqMutex<State>,
    pub waiters: KArc<Waiters>,
}

/// One end of a pty, behind a descriptor.
pub struct End {
    pub pty: KArc<Pty>,
    pub master: bool,
}

impl End {
    pub fn new(pty: KArc<Pty>, master: bool) -> End {
        {
            let mut s = pty.state.lock();
            if master {
                s.masters += 1;
            } else {
                s.slaves += 1;
            }
        }
        End { pty, master }
    }
}

impl Clone for End {
    fn clone(&self) -> End {
        End::new(self.pty.clone(), self.master)
    }
}

impl Drop for End {
    fn drop(&mut self) {
        let (hangup, gone) = {
            let mut s = self.pty.state.lock();
            s.changes += 1;
            if self.master {
                s.masters -= 1;
                (if s.masters == 0 { s.pgrp } else { 0 }, s.masters == 0)
            } else {
                s.slaves -= 1;
                (0, false)
            }
        };
        self.pty.waiters.wake_all();
        if gone {
            PTYS.lock().retain(|p| p.number != self.pty.number);
            if hangup != 0 {
                super::session::signal_group(hangup, SIGHUP);
            }
        }
    }
}

/// The ptys with a master open, by number.
static PTYS: IrqMutex<Vec<KArc<Pty>>> = IrqMutex::new(Vec::new());

/// The pty numbered `n`, while its master is open.
pub fn find(n: u32) -> Option<KArc<Pty>> {
    PTYS.lock().iter().find(|p| p.number == n).cloned()
}

/// The controlling terminal of session `sid`.
pub fn of_session(sid: u64) -> Option<KArc<Pty>> {
    if sid == 0 {
        return None;
    }
    PTYS.lock().iter().find(|p| p.state.lock().session == sid).cloned()
}

/// Session `sid`'s leader ended: its controlling terminal is no longer
/// any session's, and its foreground group gets SIGHUP (credentials(7)).
pub fn session_ended(sid: u64) {
    let Some(pty) = of_session(sid) else { return };
    let pgrp = {
        let mut s = pty.state.lock();
        let pgrp = s.pgrp;
        s.session = 0;
        s.pgrp = 0;
        pgrp
    };
    if pgrp != 0 && pgrp != sid {
        super::session::signal_group(pgrp, SIGHUP);
    }
}

/// `/dev/pts/<n>`'s number.
pub fn pts_number(path: &[u8]) -> Option<u32> {
    let digits = path.strip_prefix(b"/dev/pts/")?;
    if digits.is_empty() || digits.len() > 6 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut n = 0u32;
    for d in digits {
        n = n * 10 + u32::from(d - b'0');
    }
    find(n).map(|_| n)
}

/// A descriptor of `end`.
fn install(call: &Call<'_>, end: End, nonblock: bool, cloexec: bool) -> Result {
    let f = KArc::try_new(Fd::new(Object::Pty(end), nonblock)).map_err(|_| ENOMEM)?;
    call.lx.install_from(f, cloexec, 0)
}

/// open("/dev/ptmx"): a new pty's master (locked).
pub fn open_master(call: &Call<'_>, nonblock: bool, cloexec: bool) -> Result {
    let waiters = KArc::try_new(Waiters::default()).map_err(|_| ENOMEM)?;
    let pty = {
        let mut table = PTYS.lock();
        if table.len() >= MAX_PTYS {
            ledger::record("pty", "linux limit", MAX_PTYS as u64);
            return Err(ENOMEM);
        }
        let number = (0u32..).find(|n| table.iter().all(|p| p.number != *n)).unwrap_or(0);
        let state = State {
            ld: Ldisc::new(),
            locked: true,
            masters: 0,
            slaves: 0,
            session: 0,
            pgrp: 0,
            changes: 0,
        };
        let pty = KArc::try_new(Pty { number, state: IrqMutex::new(state), waiters })
            .map_err(|_| ENOMEM)?;
        table.try_push(pty.clone()).map_err(|_| ENOMEM)?;
        pty
    };
    install(call, End::new(pty, true), nonblock, cloexec)
}

/// open("/dev/pts/<n>"): its slave (EIO while locked). A session leader
/// without a controlling terminal opening it without O_NOCTTY takes it as
/// its own.
pub fn open_slave(call: &Call<'_>, n: u32, noctty: bool, nonblock: bool, cloexec: bool) -> Result {
    let pty = find(n).ok_or(ENXIO)?;
    if pty.state.lock().locked {
        return Err(EIO);
    }
    if !noctty {
        take_controlling(call, &pty, false);
    }
    install(call, End::new(pty, false), nonblock, cloexec)
}

/// open("/dev/tty"): the caller's controlling terminal (ENXIO without
/// one).
pub fn open_tty(call: &Call<'_>, nonblock: bool, cloexec: bool) -> Result {
    let sid = super::session::sid_of(&call.lx);
    let pty = of_session(sid).ok_or(ENXIO)?;
    install(call, End::new(pty, false), nonblock, cloexec)
}

/// Makes `pty` the controlling terminal of the caller's session if the
/// caller leads a session without one and the pty has none (`steal`:
/// TIOCSCTTY's argument 1, root only: taken from another session).
/// Whether it did.
fn take_controlling(call: &Call<'_>, pty: &Pty, steal: bool) -> bool {
    let (sid, pgid) = super::session::ids(&call.lx);
    if sid != call.lx.pid || of_session(sid).is_some() {
        return false;
    }
    let mut s = pty.state.lock();
    if s.session != 0 && !(steal && super::identity::of_process(&call.process).uid == 0) {
        return false;
    }
    s.session = sid;
    s.pgrp = pgid;
    true
}

/// The state dump's line for a pty end.
pub fn describe(pid: u64, fd: usize, end: &End) {
    let s = end.pty.state.lock();
    crate::info!(
        "lx: pid {pid} fd {fd}: pty {} {} ({} masters, {} slaves{}; {} bytes to read, {} for the master; {}{}; session {}, foreground group {}), {} waiting",
        end.pty.number,
        if end.master { "master" } else { "slave" },
        s.masters,
        s.slaves,
        if s.locked { ", locked" } else { "" },
        s.ld.readable(),
        s.ld.out.len(),
        if s.ld.t.canonical() { "canonical" } else { "raw" },
        if s.ld.t.lflag & termios::ECHO != 0 { ", echo" } else { "" },
        s.session,
        s.pgrp,
        end.pty.waiters.len()
    );
}

/// poll's events for `end`.
pub fn readiness(end: &End) -> u16 {
    use super::socket::{POLLHUP, POLLIN, POLLOUT, POLLRDNORM, POLLWRNORM};
    let s = end.pty.state.lock();
    let mut ev = 0;
    if end.master {
        if !s.ld.out.is_empty() {
            ev |= POLLIN | POLLRDNORM;
        }
        if s.slaves == 0 {
            ev |= POLLHUP;
        }
        ev |= POLLOUT | POLLWRNORM;
    } else {
        if s.ld.has_input() {
            ev |= POLLIN | POLLRDNORM;
        }
        if s.masters == 0 {
            ev |= POLLHUP | POLLIN | POLLRDNORM;
        } else if s.ld.out.len() < MAX_QUEUE {
            ev |= POLLOUT | POLLWRNORM;
        }
    }
    ev
}

pub fn changes(end: &End) -> u64 {
    end.pty.state.lock().changes
}

/// Waits for a change of the pty (until `deadline`, monotonic ns), or
/// EAGAIN for a non-blocking descriptor. Ok(true): look again; Ok(false):
/// the deadline passed.
fn wait(
    call: &Call<'_>,
    f: &Fd,
    end: &End,
    ready: impl Fn(&State) -> bool,
    deadline: Option<u64>,
) -> core::result::Result<bool, u16> {
    if f.nonblock() {
        return Err(EAGAIN);
    }
    let me = platform::current();
    end.pty.waiters.add(me).map_err(|_| ENOMEM)?;
    super::wait::prepare(call, deadline);
    if ready(&end.pty.state.lock()) {
        end.pty.waiters.remove(me);
        super::wait::abandon();
        return Ok(true);
    }
    let r = super::wait::block(call);
    end.pty.waiters.remove(me);
    r?;
    Ok(deadline.is_none_or(|d| crate::runtime::monotonic_ns() < d))
}

fn changed(end: &End, s: &mut State) {
    s.changes += 1;
    let _ = end;
}

/// read(2) of a pty end.
pub fn read(call: &Call<'_>, f: &Fd, end: &End, buf: u64, count: u64) -> Result {
    let want = (count as usize).min(MAX_QUEUE);
    let mut data = Vec::new();
    data.try_reserve_exact(want).map_err(|_| ENOMEM)?;
    data.resize(want, 0);
    let mut deadline = None;
    loop {
        if want == 0 {
            return Ok(0);
        }
        let got = {
            let mut s = end.pty.state.lock();
            if end.master {
                if !s.ld.out.is_empty() {
                    let n = s.ld.out.len().min(want);
                    for (o, b) in data.iter_mut().zip(s.ld.out.drain(..n)) {
                        *o = b;
                    }
                    Some(n)
                } else if s.slaves == 0 {
                    return Err(EIO);
                } else {
                    None
                }
            } else if let Some(n) = s.ld.take(&mut data) {
                Some(n)
            } else if s.masters == 0 {
                Some(0)
            } else if !s.ld.t.canonical() && s.ld.t.cc[termios::VMIN] == 0 {
                let tenths = u64::from(s.ld.t.cc[termios::VTIME]);
                if tenths == 0 {
                    Some(0)
                } else {
                    let d = *deadline
                        .get_or_insert(crate::runtime::monotonic_ns() + tenths * 100_000_000);
                    if crate::runtime::monotonic_ns() >= d { Some(0) } else { None }
                }
            } else {
                None
            }
        };
        if let Some(n) = got {
            if n > 0 {
                {
                    let mut s = end.pty.state.lock();
                    changed(end, &mut s);
                }
                end.pty.waiters.wake_all();
            }
            call.process.write(buf, &data[..n]).map_err(|_| EFAULT)?;
            return Ok(n as u64);
        }
        let master = end.master;
        wait(
            call,
            f,
            end,
            |s| {
                if master {
                    !s.ld.out.is_empty() || s.slaves == 0
                } else {
                    s.ld.has_input() || s.masters == 0
                }
            },
            deadline,
        )?;
    }
}

/// write(2) to a pty end: the master's bytes are typed, the slave's are
/// output.
pub fn write(call: &Call<'_>, f: &Fd, end: &End, buf: u64, count: u64) -> Result {
    let n = (count as usize).min(MAX_QUEUE);
    let mut data = Vec::new();
    data.try_reserve_exact(n).map_err(|_| ENOMEM)?;
    data.resize(n, 0);
    call.process.read(buf, &mut data).map_err(|_| EFAULT)?;
    write_bytes(call, f, end, &data)
}

/// Writes `data` (from the kernel: writev's gathered pieces) to a pty end.
pub fn write_bytes(call: &Call<'_>, f: &Fd, end: &End, data: &[u8]) -> Result {
    let n = data.len();
    if end.master {
        for &c in data {
            let (typed, pgrp) = {
                let mut s = end.pty.state.lock();
                let typed = s.ld.type_char(c)?;
                changed(end, &mut s);
                (typed, s.pgrp)
            };
            match typed {
                Typed::Nothing => {}
                Typed::FlowControl => {
                    ledger::record("pty", "linux option missing", u64::from(c));
                }
                Typed::Signal(sig) => {
                    end.pty.waiters.wake_all();
                    if pgrp != 0 {
                        if sig == SIGTSTP {
                            if super::session::group_orphaned(pgrp) {
                                continue;
                            }
                            ledger::record("pty", "linux job control stop", u64::from(sig));
                        }
                        super::session::signal_group(pgrp, sig);
                    }
                }
            }
        }
        end.pty.waiters.wake_all();
        return Ok(n as u64);
    }
    loop {
        let done = {
            let mut s = end.pty.state.lock();
            if s.masters == 0 {
                return Err(EIO);
            }
            if s.ld.out.len() < MAX_QUEUE {
                s.ld.write(data)?;
                changed(end, &mut s);
                true
            } else {
                false
            }
        };
        if done {
            end.pty.waiters.wake_all();
            return Ok(n as u64);
        }
        wait(call, f, end, |s| s.ld.out.len() < MAX_QUEUE || s.masters == 0, None)?;
    }
}

/// The kernel's struct termios as bytes.
pub fn termios_bytes(t: &Termios) -> [u8; 36] {
    let mut b = [0u8; 36];
    b[0..4].copy_from_slice(&t.iflag.to_le_bytes());
    b[4..8].copy_from_slice(&t.oflag.to_le_bytes());
    b[8..12].copy_from_slice(&t.cflag.to_le_bytes());
    b[12..16].copy_from_slice(&t.lflag.to_le_bytes());
    b[16] = t.line;
    b[17..].copy_from_slice(&t.cc);
    b
}

pub fn termios_from(b: &[u8; 36]) -> Termios {
    let word = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let mut cc = [0u8; termios::NCCS];
    cc.copy_from_slice(&b[17..]);
    Termios { iflag: word(0), oflag: word(4), cflag: word(8), lflag: word(12), line: b[16], cc }
}

fn read_u32(call: &Call<'_>, arg: u64) -> core::result::Result<u32, u16> {
    let mut b = [0u8; 4];
    call.process.read(arg, &mut b).map_err(|_| EFAULT)?;
    Ok(u32::from_le_bytes(b))
}

fn write_u32(call: &Call<'_>, arg: u64, v: u32) -> Result {
    call.process.write(arg, &v.to_le_bytes()).map_err(|_| EFAULT)?;
    Ok(0)
}

/// ioctl(2) on a pty end.
pub fn ioctl(call: &Call<'_>, f: &Fd, end: &End, cmd: u32, arg: u64) -> Result {
    let pty = &end.pty;
    // The slave's job-control requests are for its controlling process.
    let mine = || pty.state.lock().session == super::session::sid_of(&call.lx);
    match cmd {
        termios::TCGETS => {
            let t = pty.state.lock().ld.t;
            call.process.write(arg, &termios_bytes(&t)).map_err(|_| EFAULT)?;
            Ok(0)
        }
        termios::TCSETS | termios::TCSETSW | termios::TCSETSF => {
            let mut raw = [0u8; 36];
            call.process.read(arg, &mut raw).map_err(|_| EFAULT)?;
            {
                let mut s = pty.state.lock();
                s.ld.set(termios_from(&raw), cmd == termios::TCSETSF);
                s.changes += 1;
            }
            pty.waiters.wake_all();
            Ok(0)
        }
        termios::TIOCGWINSZ => {
            let w = pty.state.lock().ld.ws;
            let mut b = [0u8; 8];
            for (i, v) in [w.row, w.col, w.xpixel, w.ypixel].iter().enumerate() {
                b[2 * i..2 * i + 2].copy_from_slice(&v.to_le_bytes());
            }
            call.process.write(arg, &b).map_err(|_| EFAULT)?;
            Ok(0)
        }
        termios::TIOCSWINSZ => {
            let mut b = [0u8; 8];
            call.process.read(arg, &mut b).map_err(|_| EFAULT)?;
            let v = |i: usize| u16::from_le_bytes([b[2 * i], b[2 * i + 1]]);
            let w = Winsize { row: v(0), col: v(1), xpixel: v(2), ypixel: v(3) };
            let pgrp = {
                let mut s = pty.state.lock();
                let changed = s.ld.ws != w;
                s.ld.ws = w;
                if changed { s.pgrp } else { 0 }
            };
            if pgrp != 0 {
                super::session::signal_group(pgrp, SIGWINCH);
            }
            Ok(0)
        }
        termios::TIOCGPTN if end.master => write_u32(call, arg, pty.number),
        termios::TIOCSPTLCK if end.master => {
            let lock = read_u32(call, arg)?;
            pty.state.lock().locked = lock != 0;
            Ok(0)
        }
        TIOCGPTPEER if end.master => {
            if pty.state.lock().locked {
                return Err(EIO);
            }
            const O_NONBLOCK: u64 = 0o4000;
            const O_NOCTTY: u64 = 0o400;
            const O_CLOEXEC: u64 = 0o2000000;
            if arg & O_NOCTTY == 0 {
                take_controlling(call, pty, false);
            }
            install(call, End::new(pty.clone(), false), arg & O_NONBLOCK != 0, arg & O_CLOEXEC != 0)
        }
        // The master's requests, on a slave: not a terminal's (as Linux).
        termios::TIOCGPTN | termios::TIOCSPTLCK | TIOCGPTPEER => Err(ENOTTY),
        termios::FIONREAD | TIOCOUTQ => {
            let s = pty.state.lock();
            let n = if end.master == (cmd == termios::FIONREAD) {
                s.ld.out.len()
            } else {
                s.ld.readable()
            };
            drop(s);
            write_u32(call, arg, n as u32)
        }
        termios::TIOCSCTTY => {
            if take_controlling(call, pty, arg == 1) || mine() {
                Ok(0)
            } else {
                Err(EPERM)
            }
        }
        termios::TIOCNOTTY => {
            if !mine() {
                return Err(ENOTTY);
            }
            let (sid, _) = super::session::ids(&call.lx);
            if sid == call.lx.pid {
                let mut s = pty.state.lock();
                s.session = 0;
                s.pgrp = 0;
            }
            Ok(0)
        }
        termios::TIOCGPGRP => {
            if !end.master && !mine() {
                return Err(ENOTTY);
            }
            let pgrp = pty.state.lock().pgrp;
            write_u32(call, arg, pgrp as u32)
        }
        termios::TIOCSPGRP => {
            if !mine() {
                return Err(ENOTTY);
            }
            let pgrp = u64::from(read_u32(call, arg)?);
            if (pgrp as i32) < 0 {
                return Err(EINVAL);
            }
            let (sid, _) = super::session::ids(&call.lx);
            if !super::session::group_in_session(pgrp, sid) {
                return Err(EPERM);
            }
            pty.state.lock().pgrp = pgrp;
            Ok(0)
        }
        termios::TIOCGSID => {
            let sid = pty.state.lock().session;
            if sid == 0 || (!end.master && !mine()) {
                return Err(ENOTTY);
            }
            write_u32(call, arg, sid as u32)
        }
        TCFLSH => {
            // TCIFLUSH 0, TCOFLUSH 1, TCIOFLUSH 2 (the slave's view).
            let which = arg;
            if which > 2 {
                return Err(EINVAL);
            }
            let mut s = pty.state.lock();
            if which != 1 {
                s.ld.flush_input();
            }
            if which != 0 {
                s.ld.out.clear();
            }
            Ok(0)
        }
        TCSBRK | TCXONC => Ok(0),
        FIONBIO => {
            let on = read_u32(call, arg)? != 0;
            f.nonblock.store(on, core::sync::atomic::Ordering::Relaxed);
            Ok(0)
        }
        _ => {
            ledger::record("pty ioctl", "linux option missing", u64::from(cmd));
            Err(ENOTTY)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    fn typed(ld: &mut Ldisc, s: &[u8]) -> Vec<Typed> {
        s.iter().map(|&c| ld.type_char(c).unwrap()).collect()
    }

    fn echo(ld: &mut Ldisc) -> Vec<u8> {
        ld.out.drain(..).collect()
    }

    fn read(ld: &mut Ldisc, n: usize) -> Option<Vec<u8>> {
        let mut b = vec![0u8; n];
        ld.take(&mut b).map(|k| b[..k].to_vec())
    }

    /// pty_test's canonical-mode part, as Linux answered it.
    #[test]
    fn canonical_editing_as_linux() {
        let mut ld = Ldisc::new();
        typed(&mut ld, b"partial");
        assert!(!ld.has_input());
        typed(&mut ld, b"\n");
        assert_eq!(read(&mut ld, 64).unwrap(), b"partial\n");
        assert_eq!(echo(&mut ld), b"partial\r\n");
        typed(&mut ld, b"ab\x7fc\n");
        assert_eq!(read(&mut ld, 64).unwrap(), b"ac\n");
        assert_eq!(echo(&mut ld), b"ab\x08 \x08c\r\n");
        typed(&mut ld, b"xyz\x15ok\n");
        assert_eq!(read(&mut ld, 64).unwrap(), b"ok\n");
        assert_eq!(echo(&mut ld), b"xyz\x08 \x08\x08 \x08\x08 \x08ok\r\n");
        typed(&mut ld, b"one two\x17three\n");
        assert_eq!(read(&mut ld, 64).unwrap(), b"one three\n");
        assert_eq!(echo(&mut ld), b"one two\x08 \x08\x08 \x08\x08 \x08three\r\n");
        typed(&mut ld, b"cr\r");
        assert_eq!(read(&mut ld, 64).unwrap(), b"cr\n");
        echo(&mut ld);
        typed(&mut ld, b"abc\x04\x04");
        assert_eq!(read(&mut ld, 64).unwrap(), b"abc");
        assert_eq!(read(&mut ld, 64).unwrap(), b"");
        assert_eq!(echo(&mut ld), b"abc");
        typed(&mut ld, b"hello\n");
        assert_eq!(read(&mut ld, 2).unwrap(), b"he");
        assert_eq!(read(&mut ld, 64).unwrap(), b"llo\n");
        echo(&mut ld);
        typed(&mut ld, b"ctl\x01\n");
        assert_eq!(read(&mut ld, 64).unwrap(), b"ctl\x01\n");
        assert_eq!(echo(&mut ld), b"ctl^A\r\n");
        ld.write(b"out\nput").unwrap();
        assert_eq!(echo(&mut ld), b"out\r\nput");
    }

    #[test]
    fn raw_mode_and_signals() {
        let mut ld = Ldisc::new();
        let mut t = ld.t;
        t.make_raw();
        ld.set(t, true);
        typed(&mut ld, b"x\ry\x7f\x03");
        assert_eq!(read(&mut ld, 64).unwrap(), b"x\ry\x7f\x03");
        assert!(echo(&mut ld).is_empty());
        ld.write(b"a\nb").unwrap();
        assert_eq!(echo(&mut ld), b"a\nb");
        let mut ld = Ldisc::new();
        assert_eq!(typed(&mut ld, b"\x1a"), vec![Typed::Signal(SIGTSTP)]);
        assert_eq!(echo(&mut ld), b"^Z");
        assert_eq!(
            typed(&mut ld, b"ab\x03"),
            vec![Typed::Nothing, Typed::Nothing, Typed::Signal(SIGINT)]
        );
        assert_eq!(echo(&mut ld), b"ab^C");
        assert!(!ld.has_input());
        assert_eq!(typed(&mut ld, b"\x1c"), vec![Typed::Signal(SIGQUIT)]);
        assert_eq!(echo(&mut ld), b"^\\");
    }

    #[test]
    fn termios_round_trips() {
        let t = Termios::fresh_pty();
        assert_eq!(termios_from(&termios_bytes(&t)), t);
    }
}
