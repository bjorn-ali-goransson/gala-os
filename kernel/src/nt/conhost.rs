//! Console sessions (ARCHITECTURE "Console windows"): what the processes of
//! a console write, kept for the session's host (gala's conhost.exe, which
//! draws it in a window), and the keys the host queues for them. A
//! process belongs to at most one session (`Process::console`), which the
//! processes it creates inherit.

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
    pub down: bool,
    pub repeat: u16,
    pub vk: u16,
    pub scan: u16,
    pub ch: u16,
    pub control: u32,
}

pub struct Session {
    pub id: u32,
    inner: IrqMutex<Inner>,
    /// Set while a whole line is ready for a cooked read.
    pub line_ready: ObjectRef,
}

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
}

static SESSIONS: IrqMutex<Vec<KArc<Session>>> = IrqMutex::new(Vec::new());
static NEXT_ID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

impl Session {
    pub fn new() -> Result<KArc<Session>, NtStatus> {
        let mut ring = Vec::new();
        ring.try_reserve_exact(RING).map_err(|_| NtStatus::NO_MEMORY)?;
        ring.resize(RING, 0);
        let mut input = Vec::new();
        input.try_reserve_exact(MAX_INPUT).map_err(|_| NtStatus::NO_MEMORY)?;
        let mut editing = Vec::new();
        editing.try_reserve_exact(MAX_LINE).map_err(|_| NtStatus::NO_MEMORY)?;
        let mut ready = Vec::new();
        ready.try_reserve_exact(4 * MAX_LINE + 2).map_err(|_| NtStatus::NO_MEMORY)?;
        let line_ready = KObject::new(ObjectBody::Event(Event {
            kind: EventType::Notification,
            dispatcher: Dispatcher::new(0),
        }))
        .map_err(|_| NtStatus::NO_MEMORY)?;
        let id = NEXT_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        KArc::try_new(Session {
            id,
            inner: IrqMutex::new(Inner {
                ring,
                head: 0,
                len: 0,
                input,
                event: None,
                host: 0,
                editing,
                ready,
            }),
            line_ready,
        })
        .map_err(|_| NtStatus::NO_MEMORY)
    }

    /// Output from one of the session's processes; the host's event to
    /// set (by the caller, outside the lock).
    pub fn write(&self, bytes: &[u8]) -> Option<ObjectRef> {
        let mut s = self.inner.lock();
        s.append(bytes);
        s.event.clone()
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
        if s.host != 0 && s.host != host {
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
    /// full) and edits the cooked line with them, as the console's default
    /// input mode does (ENABLE_LINE_INPUT, ECHO_INPUT, PROCESSED_INPUT):
    /// characters are echoed into the output, Backspace takes the last one
    /// back, Enter finishes the line (CR LF). What to signal (outside the
    /// lock): the host's event (for the echo) and whether a line is ready.
    pub fn push_input(&self, keys: &[Key]) -> (usize, Option<ObjectRef>, bool) {
        let mut s = self.inner.lock();
        let room = MAX_INPUT - s.input.len();
        let n = keys.len().min(room);
        let mut line_done = false;
        for &k in &keys[..n] {
            let _ = s.input.try_push(k);
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
                    s.append(b"\r\n");
                    line_done = true;
                }
                0x08 => {
                    if s.editing.pop().is_some() {
                        s.append(b"\x08 \x08");
                    }
                }
                c if c >= 0x20 && s.editing.len() < MAX_LINE => {
                    let _ = s.editing.try_push(c);
                    let mut utf8 = [0u8; 4];
                    let ch = char::from_u32(u32::from(c)).unwrap_or('?');
                    let len = ch.encode_utf8(&mut utf8).len();
                    s.append(&utf8[..len]);
                }
                _ => {}
            }
        }
        (n, s.event.clone(), line_done)
    }

    /// Takes up to `out.len()` bytes of finished lines (a cooked read);
    /// how many. The line-ready event is reset when none are left.
    pub fn read_line(&self, out: &mut [u8]) -> usize {
        let (n, empty) = {
            let mut s = self.inner.lock();
            let n = out.len().min(s.ready.len());
            out[..n].copy_from_slice(&s.ready[..n]);
            s.ready.drain(..n);
            (n, s.ready.is_empty())
        };
        if empty {
            super::wait::set_signal(&self.line_ready, 0);
        }
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
    let session = Session::new()?;
    SESSIONS.lock().try_push(session.clone()).map_err(|_| NtStatus::NO_MEMORY)?;
    Ok(session)
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
        assert_eq!(s.push_input(&[k, k]).0, 2);
        assert_eq!(s.input_count(), 2);
        let mut out = [Key::default(); 4];
        assert_eq!(s.take_input(&mut out), 2);
        assert_eq!(out[0], k);
        let many = alloc::vec![k; MAX_INPUT + 5];
        assert_eq!(s.push_input(&many).0, MAX_INPUT);
    }

    #[test]
    fn cooked_input_echoes_edits_and_finishes_lines() {
        let s = Session::new().unwrap();
        let key = |ch: u8| Key { down: true, ch: u16::from(ch), ..Key::default() };
        let up = Key { down: false, ch: u16::from(b'x'), ..Key::default() };
        // "hx", Backspace, "i", a key release (ignored): no line yet.
        let (_, _, done) = s.push_input(&[key(b'h'), key(b'x'), key(8), key(b'i'), up]);
        assert!(!done);
        let mut line = [0u8; 16];
        assert_eq!(s.read_line(&mut line), 0);
        // Enter finishes it.
        assert!(s.push_input(&[key(b'\r')]).2);
        assert_eq!(s.read_line(&mut line), 4);
        assert_eq!(&line[..4], b"hi\r\n");
        // The echo: the characters, the Backspace undoing "x", CR LF.
        let mut echo = [0u8; 16];
        let n = s.read(&mut echo);
        assert_eq!(&echo[..n], b"hx\x08 \x08i\r\n");
    }
}
