//! Console sessions (ARCHITECTURE "Console windows"): what the processes of
//! a console write, kept for the session's host (gala's conhost.exe, which
//! draws it in a window), and the keys the host queues for them. A
//! process belongs to at most one session (`Process::console`), which the
//! processes it creates inherit.
//!
//! A *pseudo console* (ConPTY: CreatePseudoConsole, or Win32-OpenSSH's
//! `conhost.exe --headless`) is a session whose host is gala's conhost.exe
//! in pseudo-console mode: it carries the output to a pipe as VT text and
//! turns the VT input from another pipe into key records. Its output does
//! not go to the serial line (`conpty=trace` logs both directions), a bare
//! LF is written as CR LF (the console's processed output without
//! DISABLE_NEWLINE_AUTO_RETURN: a terminal's LF only moves down), it has a
//! size the programs see (GetConsoleScreenBufferInfo) and that the host or
//! the creator changes (ResizePseudoConsole), and closing it
//! (ClosePseudoConsole, or its host ending) ends its processes.

use super::object::{Event, EventType, KObject, ObjectBody, ObjectRef};
use super::status::NtStatus;
use super::wait::Dispatcher;
use crate::mm::heap::{KArc, TryVec};
use crate::sync::IrqMutex;
use alloc::vec::Vec;

/// The output ring's size: older bytes are dropped when it is full.
pub const RING: usize = 64 * 1024;
/// Queued key records at most.
pub const MAX_INPUT: usize = 256;

/// A key record as the host queues it (KEY_EVENT_RECORD's fields).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Key {
    /// KEY_EVENT_RECORD's bKeyDown. A WINDOW_BUFFER_SIZE_EVENT is a record
    /// with `size` set instead (gala's own encoding between the kernel and
    /// kernel32: bKeyDown 0x8000_0004, then columns and rows as u16).
    pub down: bool,
    pub size: Option<(u16, u16)>,
    pub repeat: u16,
    pub vk: u16,
    pub scan: u16,
    pub ch: u16,
    pub control: u32,
}

pub struct Session {
    pub id: u32,
    /// A pseudo console (ConPTY): output to its host's pipe only, LF as
    /// CR LF.
    pub pseudo: bool,
    /// The process that created it (0: the kernel): it may resize and
    /// close it as the host may.
    pub owner: u64,
    inner: IrqMutex<Inner>,
    /// Set while keys are queued (a reader waits on it).
    pub input_ready: ObjectRef,
    /// Set when Ctrl+C was typed (processed input): the session's
    /// processes' control-handler threads wait on it (GalaConsoleCtrl).
    pub ctrl_event: ObjectRef,
    /// When the last Ctrl+C came (raw monotonic ns): how late each
    /// process's handler thread takes it is logged.
    pub ctrl_raised_ns: core::sync::atomic::AtomicU64,
}

/// The console's input modes (SetConsoleMode) the session acts on.
pub const ENABLE_PROCESSED_INPUT: u32 = 0x1;
pub const ENABLE_LINE_INPUT: u32 = 0x2;
pub const ENABLE_ECHO_INPUT: u32 = 0x4;
/// Resizes are queued as WINDOW_BUFFER_SIZE_EVENT records only with it.
pub const ENABLE_WINDOW_INPUT: u32 = 0x8;
/// Output modes (SetConsoleMode on the output).
pub const ENABLE_PROCESSED_OUTPUT: u32 = 0x1;
pub const DISABLE_NEWLINE_AUTO_RETURN: u32 = 0x8;
/// A console window's size until its host says (columns, rows): what
/// kernel32 reported before sessions had sizes.
pub const DEFAULT_SIZE: (u16, u16) = (120, 30);

/// The longest line a cooked read collects (characters).
pub const MAX_LINE: usize = 1024;

struct Inner {
    ring: Vec<u8>,
    /// The oldest byte's index in `ring`, and how many are kept.
    head: usize,
    len: usize,
    input: Vec<Key>,
    /// The host's event, set when output arrives.
    event: Option<ObjectRef>,
    host: u64,
    /// The line being typed (UTF-16) and the finished lines waiting for a
    /// cooked read (UTF-8, each ending with CR LF).
    editing: Vec<u16>,
    ready: Vec<u8>,
    /// The input mode (ENABLE_*_INPUT; the default: processed, line, echo).
    mode: u32,
    /// How many Ctrl+C events there have been.
    ctrl_count: u32,
    /// The output mode (ENABLE_PROCESSED_OUTPUT, DISABLE_NEWLINE_AUTO_RETURN).
    output_mode: u32,
    /// The last byte written was CR (a following LF is not translated).
    after_cr: bool,
    /// Columns and rows.
    size: (u16, u16),
    /// Closed (ClosePseudoConsole, the host gone): no new host or member.
    closed: bool,
}

static SESSIONS: IrqMutex<Vec<KArc<Session>>> = IrqMutex::new(Vec::new());
static NEXT_ID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

/// `conpty=trace`: pseudo consoles' hosts log their VT traffic
/// (GalaConsoleTrace).
static TRACE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn set_tracing(on: bool) {
    TRACE.store(on, core::sync::atomic::Ordering::Relaxed);
}

pub fn tracing() -> bool {
    TRACE.load(core::sync::atomic::Ordering::Relaxed)
}

/// Bytes shown as text: printable ASCII as is, the rest (escape, control
/// characters, `\`, UTF-8 bytes) as `\xNN`, so a VT trace is one line.
pub struct Escaped<'a>(pub &'a [u8]);

impl core::fmt::Display for Escaped<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for &b in self.0 {
            if (0x20..0x7f).contains(&b) && b != b'\\' {
                core::fmt::Write::write_char(f, char::from(b))?;
            } else {
                write!(f, "\\x{b:02x}")?;
            }
        }
        Ok(())
    }
}

impl Session {
    pub fn new() -> Result<KArc<Session>, NtStatus> {
        Self::with(false, 0, DEFAULT_SIZE)
    }

    pub fn with(pseudo: bool, owner: u64, size: (u16, u16)) -> Result<KArc<Session>, NtStatus> {
        let mut ring = Vec::new();
        ring.try_reserve_exact(RING).map_err(|_| NtStatus::NO_MEMORY)?;
        ring.resize(RING, 0);
        let mut input = Vec::new();
        input.try_reserve_exact(MAX_INPUT).map_err(|_| NtStatus::NO_MEMORY)?;
        let mut editing = Vec::new();
        editing.try_reserve_exact(MAX_LINE).map_err(|_| NtStatus::NO_MEMORY)?;
        let mut ready = Vec::new();
        ready.try_reserve_exact(4 * MAX_LINE + 2).map_err(|_| NtStatus::NO_MEMORY)?;
        let input_ready = KObject::new(ObjectBody::Event(Event {
            kind: EventType::Notification,
            dispatcher: Dispatcher::new(0),
        }))
        .map_err(|_| NtStatus::NO_MEMORY)?;
        let ctrl_event = KObject::new(ObjectBody::Event(Event {
            kind: EventType::Notification,
            dispatcher: Dispatcher::new(0),
        }))
        .map_err(|_| NtStatus::NO_MEMORY)?;
        let id = NEXT_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        KArc::try_new(Session {
            id,
            pseudo,
            owner,
            inner: IrqMutex::new(Inner {
                ring,
                head: 0,
                len: 0,
                input,
                event: None,
                host: 0,
                editing,
                ready,
                mode: ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT,
                ctrl_count: 0,
                // ENABLE_PROCESSED_OUTPUT | ENABLE_WRAP_AT_EOL_OUTPUT.
                output_mode: ENABLE_PROCESSED_OUTPUT | 0x2,
                after_cr: false,
                size,
                closed: false,
            }),
            input_ready,
            ctrl_event,
            ctrl_raised_ns: core::sync::atomic::AtomicU64::new(0),
        })
        .map_err(|_| NtStatus::NO_MEMORY)
    }

    /// Output from one of the session's processes; the host's event to
    /// set (by the caller, outside the lock).
    pub fn write(&self, bytes: &[u8]) -> Option<ObjectRef> {
        let mut s = self.inner.lock();
        if self.pseudo {
            // A terminal's LF only moves down: the console's processed
            // output returns too (unless DISABLE_NEWLINE_AUTO_RETURN).
            let translate = s.output_mode & ENABLE_PROCESSED_OUTPUT != 0
                && s.output_mode & DISABLE_NEWLINE_AUTO_RETURN == 0;
            for &b in bytes {
                if translate && b == b'\n' && !s.after_cr {
                    s.append(b"\r");
                }
                s.after_cr = b == b'\r';
                s.append(&[b]);
            }
        } else {
            s.append(bytes);
        }
        s.event.clone()
    }

    /// The output mode (SetConsoleMode on the output).
    pub fn set_output_mode(&self, mode: u32) {
        self.inner.lock().output_mode = mode;
    }

    /// Columns and rows.
    pub fn size(&self) -> (u16, u16) {
        self.inner.lock().size
    }

    /// A new size (ResizePseudoConsole, the window's): queued as a
    /// WINDOW_BUFFER_SIZE_EVENT when the input mode asks for those; whether
    /// one was (the caller then sets `input_ready`).
    pub fn resize(&self, size: (u16, u16)) -> bool {
        let mut s = self.inner.lock();
        if s.size == size {
            return false;
        }
        s.size = size;
        if s.mode & ENABLE_WINDOW_INPUT != 0 && s.input.len() < MAX_INPUT {
            let record = Key { size: Some(size), ..Key::default() };
            return s.input.try_push(record).is_ok();
        }
        false
    }

    /// Closes the session: whether this call did (the first), and the
    /// host's event to set.
    pub fn close(&self) -> (bool, Option<ObjectRef>) {
        let mut s = self.inner.lock();
        let first = !s.closed;
        s.closed = true;
        (first, s.event.clone())
    }

    pub fn is_closed(&self) -> bool {
        self.inner.lock().closed
    }

    /// Takes up to `out.len()` bytes of output, oldest first; how many.
    pub fn read(&self, out: &mut [u8]) -> usize {
        let mut s = self.inner.lock();
        let n = out.len().min(s.len);
        for o in out.iter_mut().take(n) {
            *o = s.ring[s.head];
            s.head = (s.head + 1) % RING;
            s.len -= 1;
        }
        n
    }

    /// The host (process `host`) and its event; set at once if output is
    /// waiting. Only one host: another process is refused.
    pub fn set_host(&self, host: u64, event: ObjectRef) -> Result<Option<ObjectRef>, NtStatus> {
        let mut s = self.inner.lock();
        if (s.host != 0 && s.host != host) || s.closed {
            return Err(NtStatus::ACCESS_DENIED);
        }
        s.host = host;
        s.event = Some(event.clone());
        Ok((s.len > 0).then_some(event))
    }

    pub fn host(&self) -> u64 {
        self.inner.lock().host
    }

    /// Queues keys typed in the host's window (dropped when the queue is
    /// full): how many, and whether Ctrl+C came (with processed input it
    /// is not a key but a CTRL_C_EVENT). The caller sets `input_ready`
    /// and `ctrl_event` (outside the lock).
    pub fn push_input(&self, keys: &[Key]) -> (usize, bool) {
        let mut s = self.inner.lock();
        let (mut queued, mut ctrl) = (0, false);
        for &k in keys {
            if s.mode & ENABLE_PROCESSED_INPUT != 0 && k.ch == 3 {
                if k.down {
                    s.ctrl_count = s.ctrl_count.wrapping_add(1);
                    ctrl = true;
                }
                continue;
            }
            if s.input.len() < MAX_INPUT && s.input.try_push(k).is_ok() {
                queued += 1;
            }
        }
        (queued, ctrl)
    }

    /// The input mode (SetConsoleMode on the input).
    pub fn set_mode(&self, mode: u32) {
        self.inner.lock().mode = mode;
    }

    pub fn mode(&self) -> u32 {
        self.inner.lock().mode
    }

    /// How many Ctrl+C events there have been.
    pub fn ctrl_count(&self) -> u32 {
        self.inner.lock().ctrl_count
    }

    /// A cooked read's editing, as the console's default input mode does
    /// it (ENABLE_LINE_INPUT, ECHO_INPUT, PROCESSED_INPUT): the queued keys
    /// are consumed; characters are echoed into the output, Backspace takes
    /// the last one back, Enter finishes the line (CR LF) and stops there
    /// (keys after it wait for the next read). The host's event to set
    /// when something was echoed.
    /// Without ENABLE_LINE_INPUT every character is ready at once (Enter
    /// as CR alone), nothing echoed; without ENABLE_ECHO_INPUT the line is
    /// edited unseen.
    pub fn cook(&self) -> Option<ObjectRef> {
        let mut s = self.inner.lock();
        if s.mode & ENABLE_LINE_INPUT == 0 {
            let mut used = 0;
            while used < s.input.len() {
                let k = s.input[used];
                used += 1;
                if !k.down || k.ch == 0 {
                    continue;
                }
                let mut utf8 = [0u8; 4];
                let ch = char::from_u32(u32::from(k.ch)).unwrap_or('?');
                let len = ch.encode_utf8(&mut utf8).len();
                let _ = s.ready.try_extend_from_slice(&utf8[..len]);
            }
            s.input.drain(..used);
            return None;
        }
        let echo = s.mode & ENABLE_ECHO_INPUT != 0;
        let mut echoed = false;
        let mut used = 0;
        while used < s.input.len() {
            let k = s.input[used];
            used += 1;
            if !k.down || k.ch == 0 {
                continue;
            }
            match k.ch {
                0x0d => {
                    let mut utf8 = [0u8; 4];
                    let line = core::mem::take(&mut s.editing);
                    for c in char::decode_utf16(line.iter().copied()) {
                        let e = c.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut utf8);
                        let _ = s.ready.try_extend_from_slice(e.as_bytes());
                    }
                    s.editing = line;
                    s.editing.clear();
                    let _ = s.ready.try_extend_from_slice(b"\r\n");
                    if echo {
                        s.append(b"\r\n");
                        echoed = true;
                    }
                    break;
                }
                0x08 => {
                    if s.editing.pop().is_some() && echo {
                        s.append(b"\x08 \x08");
                        echoed = true;
                    }
                }
                c if c >= 0x20 && s.editing.len() < MAX_LINE => {
                    let _ = s.editing.try_push(c);
                    let mut utf8 = [0u8; 4];
                    let ch = char::from_u32(u32::from(c)).unwrap_or('?');
                    let len = ch.encode_utf8(&mut utf8).len();
                    if echo {
                        s.append(&utf8[..len]);
                        echoed = true;
                    }
                }
                _ => {}
            }
        }
        s.input.drain(..used);
        if echoed { s.event.clone() } else { None }
    }

    /// Takes up to `out.len()` bytes of finished lines (a cooked read);
    /// how many.
    pub fn read_line(&self, out: &mut [u8]) -> usize {
        let mut s = self.inner.lock();
        let n = out.len().min(s.ready.len());
        out[..n].copy_from_slice(&s.ready[..n]);
        s.ready.drain(..n);
        n
    }

    /// Resets `input_ready` if no key is queued (after the queue was
    /// emptied; checked again under the lock).
    pub fn settle(&self) {
        let empty = self.inner.lock().input.is_empty();
        if empty {
            super::wait::set_signal(&self.input_ready, 0);
        }
    }

    /// Copies up to `out.len()` queued keys, oldest first, without taking
    /// them (PeekConsoleInput); how many.
    pub fn peek_input(&self, out: &mut [Key]) -> usize {
        let s = self.inner.lock();
        let n = out.len().min(s.input.len());
        out[..n].copy_from_slice(&s.input[..n]);
        n
    }

    /// Takes up to `out.len()` queued keys, oldest first; how many.
    pub fn take_input(&self, out: &mut [Key]) -> usize {
        let mut s = self.inner.lock();
        let n = out.len().min(s.input.len());
        out[..n].copy_from_slice(&s.input[..n]);
        s.input.drain(..n);
        n
    }

    pub fn input_count(&self) -> usize {
        self.inner.lock().input.len()
    }
}

impl Inner {
    /// Bytes into the output ring (the oldest go when it is full).
    fn append(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if self.len == RING {
                self.head = (self.head + 1) % RING;
                self.len -= 1;
            }
            let at = (self.head + self.len) % RING;
            self.ring[at] = b;
            self.len += 1;
        }
    }
}

/// A new session, kept until the kernel stops (sessions are few: one per
/// console window).
pub fn create() -> Result<KArc<Session>, NtStatus> {
    create_with(false, 0, DEFAULT_SIZE)
}

/// A new session (`pseudo`: a pseudo console created by process `owner`);
/// closed sessions nothing refers to any more are forgotten first (a
/// pseudo console per ssh login would otherwise add up).
pub fn create_with(pseudo: bool, owner: u64, size: (u16, u16)) -> Result<KArc<Session>, NtStatus> {
    let session = Session::with(pseudo, owner, size)?;
    let mut sessions = SESSIONS.lock();
    sessions.retain(|s| !(s.is_closed() && KArc::strong_count(s) == 1));
    sessions.try_push(session.clone()).map_err(|_| NtStatus::NO_MEMORY)?;
    Ok(session)
}

/// Closes session `s` (ClosePseudoConsole, its host gone): its processes
/// other than the host and the creator end with STATUS_CONTROL_C_EXIT,
/// as a closed console's do on Windows (CTRL_CLOSE_EVENT, then the end);
/// how many.
pub fn close(s: &KArc<Session>) -> usize {
    let (first, event) = s.close();
    if let Some(e) = event {
        super::wait::set_signal(&e, 1);
    }
    // Readers waiting for keys look again (and find the session closed).
    super::wait::set_signal(&s.input_ready, 1);
    if !first {
        return 0;
    }
    let host = s.host();
    let Ok(running) = super::process::running() else { return 0 };
    let mut ended = 0;
    for p in running.iter() {
        let member = p.console.as_ref().is_some_and(|c| KArc::ptr_eq(c, s));
        if member && p.id != host && p.id != s.owner {
            super::process::terminate(p, NtStatus::CONTROL_C_EXIT, false);
            ended += 1;
        }
    }
    crate::events::emit(
        crate::events::Category::Console,
        "console close",
        format_args!(r#""id":{},"pseudo":{},"ended":{ended}"#, s.id, s.pseudo),
    );
    ended
}

/// A process ended: the sessions it hosted close (their programs end, as
/// when Windows' console host goes).
pub fn process_ended(pid: u64) {
    let hosted: Option<KArc<Session>> = {
        let sessions = SESSIONS.lock();
        sessions.iter().find(|s| s.host() == pid && !s.is_closed()).cloned()
    };
    if let Some(s) = hosted {
        crate::info!("console: session {} closes: its host (pid {pid}) ended", s.id);
        close(&s);
    }
}

pub fn find(id: u32) -> Option<KArc<Session>> {
    SESSIONS.lock().iter().find(|s| s.id == id).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_kept_in_order_and_the_oldest_goes_when_full() {
        let s = Session::new().unwrap();
        assert!(s.write(b"hello ").is_none());
        s.write(b"world");
        let mut out = [0u8; 8];
        assert_eq!(s.read(&mut out), 8);
        assert_eq!(&out, b"hello wo");
        assert_eq!(s.read(&mut out), 3);
        assert_eq!(&out[..3], b"rld");
        assert_eq!(s.read(&mut out), 0);
        // More than the ring holds: the last RING bytes stay.
        let big: Vec<u8> = (0..RING + 10).map(|i| (i % 251) as u8).collect();
        s.write(&big);
        let mut all = alloc::vec![0u8; RING + 10];
        assert_eq!(s.read(&mut all), RING);
        assert_eq!(&all[..RING], &big[10..]);
    }

    #[test]
    fn keys_queue_up_to_the_limit() {
        let s = Session::new().unwrap();
        let k = Key { down: true, vk: 0x41, ch: u16::from(b'a'), ..Key::default() };
        assert_eq!(s.push_input(&[k, k]), (2, false));
        assert_eq!(s.input_count(), 2);
        let mut out = [Key::default(); 4];
        assert_eq!(s.take_input(&mut out), 2);
        assert_eq!(out[0], k);
        let many = alloc::vec![k; MAX_INPUT + 5];
        assert_eq!(s.push_input(&many), (MAX_INPUT, false));
    }

    #[test]
    fn cooked_input_echoes_edits_and_finishes_lines() {
        let s = Session::new().unwrap();
        let key = |ch: u8| Key { down: true, ch: u16::from(ch), ..Key::default() };
        let up = Key { down: false, ch: u16::from(b'x'), ..Key::default() };
        // "hx", Backspace, "i", a key release (ignored): no line yet.
        s.push_input(&[key(b'h'), key(b'x'), key(8), key(b'i'), up]);
        s.cook();
        let mut line = [0u8; 16];
        assert_eq!(s.read_line(&mut line), 0);
        assert_eq!(s.input_count(), 0);
        // Enter finishes it; what follows waits for the next read.
        s.push_input(&[key(b'\r'), key(b'z')]);
        s.cook();
        assert_eq!(s.read_line(&mut line), 4);
        assert_eq!(&line[..4], b"hi\r\n");
        assert_eq!(s.input_count(), 1);
        // The echo: the characters, the Backspace undoing "x", CR LF.
        let mut echo = [0u8; 16];
        let n = s.read(&mut echo);
        assert_eq!(&echo[..n], b"hx\x08 \x08i\r\n");
        // Raw readers see the records (PeekConsoleInput keeps them).
        let mut keys = [Key::default(); 4];
        assert_eq!(s.peek_input(&mut keys), 1);
        assert_eq!(s.take_input(&mut keys), 1);
        assert_eq!(keys[0].ch, u16::from(b'z'));
    }

    #[test]
    fn modes_turn_off_line_editing_and_echo_and_ctrl_c_is_an_event() {
        let s = Session::new().unwrap();
        let key = |ch: u16| Key { down: true, ch, ..Key::default() };
        // Processed input: Ctrl+C is counted, not queued.
        assert_eq!(s.push_input(&[key(3), key(u16::from(b'a'))]), (1, true));
        assert_eq!(s.ctrl_count(), 1);
        // Line input and echo off: each character at once, Enter as CR,
        // nothing echoed.
        s.set_mode(ENABLE_PROCESSED_INPUT);
        s.push_input(&[key(u16::from(b'b')), key(13)]);
        assert!(s.cook().is_none());
        let mut out = [0u8; 8];
        assert_eq!(s.read_line(&mut out), 3);
        assert_eq!(&out[..3], b"ab\r");
        assert_eq!(s.read(&mut out), 0);
        // Echo off with line input: edited unseen, finished by Enter.
        s.set_mode(ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT);
        s.push_input(&[key(u16::from(b'q')), key(13)]);
        s.cook();
        assert_eq!(s.read_line(&mut out), 3);
        assert_eq!(s.read(&mut out), 0);
        // Not processed: Ctrl+C is a character.
        s.set_mode(0);
        assert_eq!(s.push_input(&[key(3)]), (1, false));
        assert_eq!(s.ctrl_count(), 1);
    }

    #[test]
    fn a_pseudo_console_returns_at_line_feeds_and_resizes() {
        let s = Session::with(true, 7, (80, 25)).unwrap();
        // A bare LF becomes CR LF; CR LF stays (also across writes).
        s.write(b"a\nb\r");
        s.write(b"\nc\n");
        let mut out = [0u8; 16];
        let n = s.read(&mut out);
        assert_eq!(&out[..n], b"a\r\nb\r\nc\r\n");
        // DISABLE_NEWLINE_AUTO_RETURN: LF alone.
        s.set_output_mode(ENABLE_PROCESSED_OUTPUT | DISABLE_NEWLINE_AUTO_RETURN);
        s.write(b"d\n");
        let n = s.read(&mut out);
        assert_eq!(&out[..n], b"d\n");
        // A window's session is not translated.
        let w = Session::new().unwrap();
        w.write(b"e\n");
        assert_eq!(w.read(&mut out), 2);
        // Resizes: the size, and a record only with ENABLE_WINDOW_INPUT.
        assert_eq!(s.size(), (80, 25));
        assert!(!s.resize((100, 30)));
        assert_eq!(s.size(), (100, 30));
        s.set_mode(ENABLE_WINDOW_INPUT);
        assert!(s.resize((90, 20)));
        assert!(!s.resize((90, 20)));
        let mut keys = [Key::default(); 2];
        assert_eq!(s.take_input(&mut keys), 1);
        assert_eq!(keys[0].size, Some((90, 20)));
        // Closing: once; a closed session takes no host.
        assert!(s.close().0);
        assert!(!s.close().0);
        let event = KObject::new(ObjectBody::Event(Event {
            kind: EventType::Notification,
            dispatcher: Dispatcher::new(0),
        }))
        .unwrap();
        assert!(s.set_host(9, event).is_err());
    }
}
