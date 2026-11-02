//! The window registry: every window of every process (top-level and
//! child), as Windows' win32k keeps them, so that one process can find
//! and ask about another's windows (FindWindow, EnumWindows,
//! GetWindowText, UI Automation). user32 keeps its own table as the fast
//! path, registers each window here when it creates it (the HWND comes
//! from here: one namespace for the desktop) and mirrors every change.
//! See docs/ARCHITECTURE.md, "Plan: windows across processes".

use super::object::{Event, EventType, KObject, ObjectBody, ObjectRef};
use super::status::NtStatus;
use super::wait::Dispatcher;
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;
use alloc::vec::Vec;

/// A window's record as user32 hands it over and reads it back
/// (GalaWindowRegister/Update/Query): little-endian, INFO_SIZE bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub hwnd: u64,
    pub parent: u64,
    pub owner: u64,
    /// The control id (a child's), or the menu handle (a top-level's).
    pub id: u64,
    pub pid: u32,
    pub tid: u32,
    pub style: u32,
    pub ex_style: u32,
    /// The window rectangle on the screen: left, top, right, bottom.
    pub rect: [i32; 4],
    /// FLAG_VISIBLE, FLAG_ENABLED.
    pub flags: u32,
    /// Z order among siblings: smaller is higher.
    pub z: i64,
    pub class_len: u32,
    pub class: [u16; CLASS_CHARS],
    pub text_len: u32,
    pub text: [u16; TEXT_CHARS],
}

pub const CLASS_CHARS: usize = 64;
pub const TEXT_CHARS: usize = 256;
pub const INFO_SIZE: usize = 88 + 2 * CLASS_CHARS + 2 * TEXT_CHARS;
/// The record's format version (bytes 84..88; ARCHITECTURE invariant 3:
/// everything shared is versioned). Readers accept this version only for
/// now; a new one adds fields at the end and keeps accepting the old.
pub const INFO_VERSION: u32 = 1;
pub const FLAG_VISIBLE: u32 = 1;
pub const FLAG_ENABLED: u32 = 2;

/// The first HWND handed out and the step between them (Windows' handles
/// are even; user32's own pseudo handles, such as the desktop's, stay
/// below).
const FIRST_HWND: u64 = 0x0002_0000;
const HWND_STEP: u64 = 0x10;

impl Default for Info {
    fn default() -> Self {
        Info {
            hwnd: 0,
            parent: 0,
            owner: 0,
            id: 0,
            pid: 0,
            tid: 0,
            style: 0,
            ex_style: 0,
            rect: [0; 4],
            flags: 0,
            z: 0,
            class_len: 0,
            class: [0; CLASS_CHARS],
            text_len: 0,
            text: [0; TEXT_CHARS],
        }
    }
}

impl Info {
    pub fn from_bytes(b: &[u8]) -> Option<Info> {
        if b.len() < INFO_SIZE || b[84..88] != INFO_VERSION.to_le_bytes() {
            return None;
        }
        let u64_at = |at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap_or([0; 8]));
        let u32_at = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap_or([0; 4]));
        let mut info = Info {
            hwnd: u64_at(0),
            parent: u64_at(8),
            owner: u64_at(16),
            id: u64_at(24),
            pid: u32_at(32),
            tid: u32_at(36),
            style: u32_at(40),
            ex_style: u32_at(44),
            rect: [u32_at(48) as i32, u32_at(52) as i32, u32_at(56) as i32, u32_at(60) as i32],
            flags: u32_at(64),
            class_len: u32_at(68).min(CLASS_CHARS as u32),
            z: u64_at(72) as i64,
            text_len: u32_at(80).min(TEXT_CHARS as u32),
            ..Info::default()
        };
        for (i, c) in info.class.iter_mut().enumerate() {
            *c = u16::from_le_bytes([b[88 + 2 * i], b[89 + 2 * i]]);
        }
        let text = 88 + 2 * CLASS_CHARS;
        for (i, c) in info.text.iter_mut().enumerate() {
            *c = u16::from_le_bytes([b[text + 2 * i], b[text + 1 + 2 * i]]);
        }
        Some(info)
    }

    pub fn to_bytes(&self, b: &mut [u8; INFO_SIZE]) {
        b[0..8].copy_from_slice(&self.hwnd.to_le_bytes());
        b[8..16].copy_from_slice(&self.parent.to_le_bytes());
        b[16..24].copy_from_slice(&self.owner.to_le_bytes());
        b[24..32].copy_from_slice(&self.id.to_le_bytes());
        b[32..36].copy_from_slice(&self.pid.to_le_bytes());
        b[36..40].copy_from_slice(&self.tid.to_le_bytes());
        b[40..44].copy_from_slice(&self.style.to_le_bytes());
        b[44..48].copy_from_slice(&self.ex_style.to_le_bytes());
        for (i, v) in self.rect.iter().enumerate() {
            b[48 + 4 * i..52 + 4 * i].copy_from_slice(&v.to_le_bytes());
        }
        b[64..68].copy_from_slice(&self.flags.to_le_bytes());
        b[68..72].copy_from_slice(&self.class_len.to_le_bytes());
        b[72..80].copy_from_slice(&self.z.to_le_bytes());
        b[80..84].copy_from_slice(&self.text_len.to_le_bytes());
        b[84..88].copy_from_slice(&INFO_VERSION.to_le_bytes());
        for (i, c) in self.class.iter().enumerate() {
            b[88 + 2 * i..90 + 2 * i].copy_from_slice(&c.to_le_bytes());
        }
        let text = 88 + 2 * CLASS_CHARS;
        for (i, c) in self.text.iter().enumerate() {
            b[text + 2 * i..text + 2 + 2 * i].copy_from_slice(&c.to_le_bytes());
        }
    }
}

struct Registry {
    windows: Vec<Info>,
    next: u64,
    /// Messages posted to another process's window, waiting for it.
    posted: Vec<Posted>,
    /// Each process's queue event (GalaWindowListen), set when a message
    /// is posted or sent to it.
    listeners: Vec<(u32, ObjectRef)>,
    /// Messages sent to another process's window, until their sender
    /// has the reply.
    sent: Vec<Sent>,
    next_sent: u64,
}

/// A message sent to window `hwnd` of process `to_pid`, thread `to_tid`
/// (the window's), by process `from_pid`: what the receiver gets (`data`:
/// the marshaled input, `out_max`: room for the output) and the reply.
pub struct Sent {
    pub id: u64,
    pub from_pid: u32,
    pub to_pid: u32,
    pub to_tid: u32,
    pub hwnd: u64,
    pub message: u32,
    pub wparam: u64,
    pub lparam: u64,
    pub data: Vec<u8>,
    pub out_max: u32,
    /// Handed to the receiver.
    pub taken: bool,
    /// The result and the output, once replied; None while waiting.
    pub reply: Option<(i64, Vec<u8>)>,
    /// The receiver is gone without replying.
    pub failed: bool,
    /// Set when replied or failed.
    pub done: ObjectRef,
}

/// The most input or output a sent message carries (bytes).
pub const MAX_SENT_DATA: usize = 64 * 1024;

/// A message posted to process `pid`'s window `hwnd`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Posted {
    pub pid: u32,
    pub hwnd: u64,
    pub message: u32,
    pub wparam: u64,
    pub lparam: u64,
}

/// At most this many posted messages wait (Windows' default limit per
/// queue is 10000; posting beyond fails).
pub const MAX_POSTED: usize = 4096;

static REGISTRY: IrqMutex<Registry> = IrqMutex::new(Registry {
    windows: Vec::new(),
    next: 0,
    posted: Vec::new(),
    listeners: Vec::new(),
    sent: Vec::new(),
    next_sent: 1,
});

/// Queues a message sent by process `from_pid` to window `hwnd`: its id,
/// the reply event to wait on, and the receiver's queue event to set.
#[allow(clippy::too_many_arguments)]
pub fn send(
    from_pid: u32,
    hwnd: u64,
    message: u32,
    wparam: u64,
    lparam: u64,
    data: Vec<u8>,
    out_max: u32,
) -> Result<(u64, ObjectRef, Option<ObjectRef>), NtStatus> {
    let done = KObject::new(ObjectBody::Event(Event {
        kind: EventType::Notification,
        dispatcher: Dispatcher::new(0),
    }))
    .map_err(|_| NtStatus::NO_MEMORY)?;
    let mut r = REGISTRY.lock();
    let (to_pid, to_tid) = r
        .windows
        .iter()
        .find(|w| w.hwnd == hwnd)
        .map(|w| (w.pid, w.tid))
        .ok_or(NtStatus::INVALID_HANDLE)?;
    r.sent.try_reserve(1).map_err(|_| NtStatus::NO_MEMORY)?;
    let id = r.next_sent;
    r.next_sent += 1;
    let out_max = out_max.min(MAX_SENT_DATA as u32);
    r.sent
        .try_push(Sent {
            id,
            from_pid,
            to_pid,
            to_tid,
            hwnd,
            message,
            wparam,
            lparam,
            data,
            out_max,
            taken: false,
            reply: None,
            failed: false,
            done: done.clone(),
        })
        .map_err(|_| NtStatus::NO_MEMORY)?;
    let wake = r.listeners.iter().find(|(p, _)| *p == to_pid).map(|(_, e)| e.clone());
    Ok((id, done, wake))
}

/// The reply event of `from_pid`'s sent message `id`.
pub fn sent_event(from_pid: u32, id: u64) -> Option<ObjectRef> {
    let r = REGISTRY.lock();
    r.sent.iter().find(|m| m.id == id && m.from_pid == from_pid).map(|m| m.done.clone())
}

/// What became of `from_pid`'s sent message `id`: Some(Ok(result,
/// output)) once replied (the entry goes), Some(Err) when the receiver
/// is gone (the entry goes), None while waiting.
pub fn take_reply(from_pid: u32, id: u64) -> Option<Result<(i64, Vec<u8>), NtStatus>> {
    let mut r = REGISTRY.lock();
    let at = r.sent.iter().position(|m| m.id == id && m.from_pid == from_pid);
    let Some(at) = at else { return Some(Err(NtStatus::INVALID_HANDLE)) };
    let m = &r.sent[at];
    if m.failed {
        r.sent.remove(at);
        return Some(Err(NtStatus::INVALID_HANDLE));
    }
    m.reply.as_ref()?;
    let m = r.sent.remove(at);
    m.reply.map(Ok)
}

/// A sent message as its receiver gets it: (id, hwnd, message, wParam,
/// lParam, input, room for output).
pub type Received = (u64, u64, u32, u64, u64, Vec<u8>, u32);

/// Hands the oldest message sent to thread `tid` of process `pid` to it
/// (taken: it replies with `reply`): (id, hwnd, message, wParam, lParam,
/// input, room for output).
pub fn receive(pid: u32, tid: u32) -> Option<Received> {
    let mut r = REGISTRY.lock();
    let m = r.sent.iter_mut().find(|m| m.to_pid == pid && m.to_tid == tid && !m.taken)?;
    m.taken = true;
    let data = core::mem::take(&mut m.data);
    Some((m.id, m.hwnd, m.message, m.wparam, m.lparam, data, m.out_max))
}

/// Whether a message sent to thread `tid` of process `pid` waits.
pub fn sent_waiting(pid: u32, tid: u32) -> bool {
    REGISTRY.lock().sent.iter().any(|m| m.to_pid == pid && m.to_tid == tid && !m.taken)
}

/// Process `pid` replies to sent message `id`; the sender's event to set.
pub fn reply(pid: u32, id: u64, result: i64, out: Vec<u8>) -> Result<ObjectRef, NtStatus> {
    let mut r = REGISTRY.lock();
    let m = r
        .sent
        .iter_mut()
        .find(|m| m.id == id && m.to_pid == pid && m.taken && m.reply.is_none())
        .ok_or(NtStatus::INVALID_PARAMETER)?;
    m.reply = Some((result, out));
    Ok(m.done.clone())
}

/// `pid`'s queue event, set when messages are posted to it.
pub fn listen(pid: u32, event: ObjectRef) -> Result<(), NtStatus> {
    let mut r = REGISTRY.lock();
    r.listeners.retain(|(p, _)| *p != pid);
    r.listeners.try_push((pid, event)).map_err(|_| NtStatus::NO_MEMORY)
}

/// Posts a message to window `hwnd` (any process's); the owner's event
/// to set (by the caller, outside the lock).
pub fn post(
    hwnd: u64,
    message: u32,
    wparam: u64,
    lparam: u64,
) -> Result<Option<ObjectRef>, NtStatus> {
    let mut r = REGISTRY.lock();
    let pid = r.windows.iter().find(|w| w.hwnd == hwnd).ok_or(NtStatus::INVALID_HANDLE)?.pid;
    if r.posted.len() >= MAX_POSTED {
        return Err(NtStatus::INSUFFICIENT_RESOURCES);
    }
    r.posted
        .try_push(Posted { pid, hwnd, message, wparam, lparam })
        .map_err(|_| NtStatus::NO_MEMORY)?;
    Ok(r.listeners.iter().find(|(p, _)| *p == pid).map(|(_, e)| e.clone()))
}

/// Takes up to `out.len()` of the messages posted to `pid`'s windows,
/// oldest first; how many.
pub fn take_posted(pid: u32, out: &mut [Posted]) -> usize {
    let mut r = REGISTRY.lock();
    let mut n = 0;
    r.posted.retain(|m| {
        if m.pid == pid && n < out.len() {
            out[n] = *m;
            n += 1;
            false
        } else {
            true
        }
    });
    n
}

/// Registers a window of process `pid`; its new HWND.
pub fn register(pid: u32, mut info: Info) -> Result<u64, NtStatus> {
    let mut r = REGISTRY.lock();
    r.windows.try_reserve(1).map_err(|_| NtStatus::NO_MEMORY)?;
    let hwnd = FIRST_HWND + r.next * HWND_STEP;
    r.next += 1;
    info.hwnd = hwnd;
    info.pid = pid;
    r.windows.try_push(info).map_err(|_| NtStatus::NO_MEMORY)?;
    Ok(hwnd)
}

/// Replaces the record of `pid`'s window `info.hwnd`.
pub fn update(pid: u32, mut info: Info) -> Result<(), NtStatus> {
    let mut r = REGISTRY.lock();
    let w = r
        .windows
        .iter_mut()
        .find(|w| w.hwnd == info.hwnd && w.pid == pid)
        .ok_or(NtStatus::INVALID_HANDLE)?;
    info.pid = pid;
    *w = info;
    Ok(())
}

/// `pid`'s window `hwnd` is gone.
pub fn unregister(pid: u32, hwnd: u64) -> Result<(), NtStatus> {
    let mut r = REGISTRY.lock();
    let before = r.windows.len();
    r.windows.retain(|w| !(w.hwnd == hwnd && w.pid == pid));
    if r.windows.len() == before { Err(NtStatus::INVALID_HANDLE) } else { Ok(()) }
}

/// Any process's window `hwnd`.
pub fn query(hwnd: u64) -> Option<Info> {
    REGISTRY.lock().windows.iter().find(|w| w.hwnd == hwnd).copied()
}

/// The windows whose parent is `parent` (0: the top-level windows), the
/// highest first, into `out`; how many there are (more than `out` holds
/// when it is short).
pub fn children(parent: u64, out: &mut [u64]) -> usize {
    let r = REGISTRY.lock();
    let mut found: Vec<(i64, u64)> = Vec::new();
    for w in r.windows.iter().filter(|w| w.parent == parent) {
        if found.try_push((w.z, w.hwnd)).is_err() {
            break;
        }
    }
    drop(r);
    found.sort_unstable();
    for (o, (_, h)) in out.iter_mut().zip(&found) {
        *o = *h;
    }
    found.len()
}

/// A process ended: its windows go.
pub fn process_ended(pid: u32) {
    let mut wake: Vec<ObjectRef> = Vec::new();
    let mut r = REGISTRY.lock();
    r.windows.retain(|w| w.pid != pid);
    r.posted.retain(|m| m.pid != pid);
    r.listeners.retain(|(p, _)| *p != pid);
    // Its own sends go; sends to it fail (their senders wake).
    r.sent.retain(|m| m.from_pid != pid);
    for m in r.sent.iter_mut().filter(|m| m.to_pid == pid) {
        m.failed = true;
        let _ = wake.try_push(m.done.clone());
    }
    drop(r);
    for e in &wake {
        super::wait::set_signal(e, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_and_windows_are_found_by_anyone() {
        let mut info = Info {
            parent: 0,
            style: 0x10cf_0000,
            rect: [10, 20, 310, 220],
            flags: FLAG_VISIBLE | FLAG_ENABLED,
            z: -5,
            class_len: 3,
            text_len: 2,
            tid: 77,
            ..Info::default()
        };
        info.class[..3].copy_from_slice(&[u16::from(b'A'), u16::from(b'b'), u16::from(b'c')]);
        info.text[..2].copy_from_slice(&[u16::from(b'h'), u16::from(b'i')]);
        let mut bytes = [0u8; INFO_SIZE];
        info.to_bytes(&mut bytes);
        assert_eq!(Info::from_bytes(&bytes), Some(info));
        // Another format version is refused.
        bytes[84] = 2;
        assert_eq!(Info::from_bytes(&bytes), None);

        let top = register(901, info).unwrap();
        assert_eq!(top % HWND_STEP, 0);
        let child_info = Info { parent: top, id: 101, z: 2, ..Info::default() };
        let b = register(901, child_info).unwrap();
        let a = register(901, Info { z: 1, ..child_info }).unwrap();
        assert_ne!(a, b);
        // Another process reads them.
        let q = query(top).unwrap();
        assert_eq!((q.pid, q.tid, q.rect, q.text_len), (901, 77, [10, 20, 310, 220], 2));
        let mut kids = [0u64; 1];
        assert_eq!(children(top, &mut kids), 2);
        assert_eq!(kids[0], a); // the higher (smaller z) first
        // Only the owner changes or removes them.
        let moved = Info { rect: [0, 0, 5, 5], ..q };
        assert_eq!(update(902, moved), Err(NtStatus::INVALID_HANDLE));
        update(901, moved).unwrap();
        assert_eq!(query(top).unwrap().rect, [0, 0, 5, 5]);
        assert_eq!(unregister(902, a), Err(NtStatus::INVALID_HANDLE));
        unregister(901, a).unwrap();
        assert_eq!(children(top, &mut kids), 1);
        // A message posted to its window waits for its owner.
        assert!(post(b, 0x8001, 7, 9).unwrap().is_none());
        assert!(matches!(post(a, 0x8001, 0, 0), Err(NtStatus::INVALID_HANDLE)));
        let mut got = [Posted { pid: 0, hwnd: 0, message: 0, wparam: 0, lparam: 0 }; 4];
        assert_eq!(take_posted(902, &mut got), 0);
        assert_eq!(take_posted(901, &mut got), 1);
        assert_eq!(got[0], Posted { pid: 901, hwnd: b, message: 0x8001, wparam: 7, lparam: 9 });
        assert_eq!(take_posted(901, &mut got), 0);
        // A sent message: handed to the window's thread, replied to.
        let (id, done, wake) = send(902, b, 0x000d, 4, 0, Vec::new(), 8).unwrap();
        assert!(wake.is_none());
        assert!(take_reply(902, id).is_none());
        assert!(receive(901, 76).is_none()); // another thread
        let got = receive(901, 0).unwrap();
        assert_eq!((got.0, got.1, got.2, got.6), (id, b, 0x000d, 8));
        assert!(receive(901, 0).is_none());
        let mut out = Vec::new();
        out.try_extend_from_slice(b"hi").unwrap();
        reply(901, id, 2, out).unwrap();
        drop(done);
        let (result, data) = take_reply(902, id).unwrap().unwrap();
        assert_eq!((result, &data[..]), (2, &b"hi"[..]));
        // A receiver that ends fails its senders.
        let (id, _, _) = send(902, b, 0x8001, 0, 0, Vec::new(), 0).unwrap();
        process_ended(901);
        assert_eq!(take_reply(902, id).map(|r| r.is_err()), Some(true));
        assert!(query(top).is_none() && query(b).is_none());
    }
}
