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
    /// FLAG_VISIBLE, FLAG_ENABLED, FLAG_ZOOMED, FLAG_ICONIC (kept as
    /// user32 writes them).
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
/// Maximized (IsZoomed) and minimized (IsIconic) top-level windows.
pub const FLAG_ZOOMED: u32 = 4;
pub const FLAG_ICONIC: u32 = 8;

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
    /// Messages posted to another process's window or thread, waiting
    /// for it.
    posted: Vec<Posted>,
    /// Each process's queue event (GalaWindowListen), set when a message
    /// is posted or sent to it.
    listeners: Vec<(u32, ObjectRef)>,
    /// Messages sent to another thread's window, until their sender has
    /// the reply.
    sent: Vec<Sent>,
    next_sent: u64,
    /// The threads that have a message queue, (pid, tid): those that
    /// created a window or called a message function (PostThreadMessage
    /// fails for the others, as on Windows).
    queues: Vec<(u32, u32)>,
}

/// A message sent to window `hwnd` of process `to_pid`, thread `to_tid`
/// (the window's), by thread `from_tid` of process `from_pid`: what the
/// receiver gets (`data`: the marshaled input, `out_max`: room for the
/// output) and the reply.
pub struct Sent {
    pub id: u64,
    pub from_pid: u32,
    pub from_tid: u32,
    pub to_pid: u32,
    pub to_tid: u32,
    pub hwnd: u64,
    pub message: u32,
    pub wparam: u64,
    pub lparam: u64,
    pub data: Vec<u8>,
    pub out_max: u32,
    /// SEND_NOTIFY, SEND_CALLBACK, SEND_ABORT_IF_HUNG.
    pub flags: u32,
    /// When it was sent (ms, the caller's clock): a receiver that leaves
    /// a message waiting HUNG_MS is hung.
    pub queued_at: u64,
    /// Handed to the receiver.
    pub taken: bool,
    /// The result and the output, once replied; None while waiting.
    pub reply: Option<(i64, Vec<u8>)>,
    /// The receiver is gone without replying.
    pub failed: bool,
    /// The sender stopped waiting (SendMessageTimeout's time-out): the
    /// reply goes nowhere.
    pub abandoned: bool,
    /// Set when replied or failed.
    pub done: ObjectRef,
}

/// SendNotifyMessage: nobody waits for the reply (the entry goes when
/// the receiver is done).
pub const SEND_NOTIFY: u32 = 1;
/// SendMessageCallback: the reply sets the sender's queue event (its
/// pump calls the callback).
pub const SEND_CALLBACK: u32 = 2;
/// SendMessageTimeout's SMTO_ABORTIFHUNG: fails at once (STATUS_TIMEOUT)
/// when the receiver is hung.
pub const SEND_ABORT_IF_HUNG: u32 = 4;
/// A thread that leaves a sent message waiting this long is hung (Windows
/// documents 5 seconds for IsHungAppWindow and SMTO_ABORTIFHUNG).
pub const HUNG_MS: u64 = 5000;

/// The most input or output a sent message carries (bytes).
pub const MAX_SENT_DATA: usize = 64 * 1024;

/// A message posted to process `pid`'s window `hwnd`, or to its thread
/// `tid` (hwnd 0: PostThreadMessage).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Posted {
    pub pid: u32,
    pub tid: u32,
    pub hwnd: u64,
    pub message: u32,
    pub wparam: u64,
    pub lparam: u64,
}

impl Posted {
    pub const EMPTY: Posted = Posted { pid: 0, tid: 0, hwnd: 0, message: 0, wparam: 0, lparam: 0 };
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
    queues: Vec::new(),
});

/// A message to send (GalaWindowSend): from thread `from_tid` of process
/// `from_pid` to window `hwnd`, its marshaled input and the room for its
/// output; `now` is the caller's clock (ms).
pub struct Request {
    pub from_pid: u32,
    pub from_tid: u32,
    pub hwnd: u64,
    pub message: u32,
    pub wparam: u64,
    pub lparam: u64,
    pub data: Vec<u8>,
    pub out_max: u32,
    pub flags: u32,
    pub now: u64,
}

/// Queues a sent message for its window's thread: its id and the
/// receiver's queue event to set. STATUS_INVALID_HANDLE for no window,
/// STATUS_TIMEOUT for a hung receiver with SEND_ABORT_IF_HUNG.
pub fn send(req: Request) -> Result<(u64, Option<ObjectRef>), NtStatus> {
    let done = KObject::new(ObjectBody::Event(Event {
        kind: EventType::Notification,
        dispatcher: Dispatcher::new(0),
    }))
    .map_err(|_| NtStatus::NO_MEMORY)?;
    let mut r = REGISTRY.lock();
    let (to_pid, to_tid) = r
        .windows
        .iter()
        .find(|w| w.hwnd == req.hwnd)
        .map(|w| (w.pid, w.tid))
        .ok_or(NtStatus::INVALID_HANDLE)?;
    if req.flags & SEND_ABORT_IF_HUNG != 0
        && r.sent.iter().any(|m| {
            m.to_pid == to_pid
                && m.to_tid == to_tid
                && !m.taken
                && req.now.saturating_sub(m.queued_at) >= HUNG_MS
        })
    {
        return Err(NtStatus::TIMEOUT);
    }
    r.sent.try_reserve(1).map_err(|_| NtStatus::NO_MEMORY)?;
    let id = r.next_sent;
    r.next_sent += 1;
    let out_max = req.out_max.min(MAX_SENT_DATA as u32);
    r.sent
        .try_push(Sent {
            id,
            from_pid: req.from_pid,
            from_tid: req.from_tid,
            to_pid,
            to_tid,
            hwnd: req.hwnd,
            message: req.message,
            wparam: req.wparam,
            lparam: req.lparam,
            data: req.data,
            out_max,
            flags: req.flags,
            queued_at: req.now,
            taken: false,
            reply: None,
            failed: false,
            abandoned: false,
            done,
        })
        .map_err(|_| NtStatus::NO_MEMORY)?;
    let wake = r.listeners.iter().find(|(p, _)| *p == to_pid).map(|(_, e)| e.clone());
    Ok((id, wake))
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

/// `from_pid` stops waiting for its sent message `id` (a time-out): it
/// goes now if answered, else once its receiver replies (the receiver
/// still handles it, as on Windows).
pub fn abandon(from_pid: u32, id: u64) -> Result<(), NtStatus> {
    let mut r = REGISTRY.lock();
    let at = r
        .sent
        .iter()
        .position(|m| m.id == id && m.from_pid == from_pid)
        .ok_or(NtStatus::INVALID_HANDLE)?;
    let m = &mut r.sent[at];
    if m.reply.is_some() || m.failed {
        r.sent.remove(at);
    } else {
        m.abandoned = true;
    }
    Ok(())
}

/// A sent message as its receiver gets it.
pub struct Received {
    pub id: u64,
    pub hwnd: u64,
    pub message: u32,
    pub wparam: u64,
    pub lparam: u64,
    /// The marshaled input.
    pub data: Vec<u8>,
    /// Room for the output.
    pub out_max: u32,
    /// How it was sent (SEND_NOTIFY, SEND_CALLBACK, ...).
    pub flags: u32,
}

/// Hands the oldest message sent to thread `tid` of process `pid` to it
/// (taken: it replies with `reply`).
pub fn receive(pid: u32, tid: u32) -> Option<Received> {
    let mut r = REGISTRY.lock();
    let m = r.sent.iter_mut().find(|m| m.to_pid == pid && m.to_tid == tid && !m.taken)?;
    m.taken = true;
    let data = core::mem::take(&mut m.data);
    Some(Received {
        id: m.id,
        hwnd: m.hwnd,
        message: m.message,
        wparam: m.wparam,
        lparam: m.lparam,
        data,
        out_max: m.out_max,
        flags: m.flags,
    })
}

/// Whether a message sent to thread `tid` of process `pid` waits.
pub fn sent_waiting(pid: u32, tid: u32) -> bool {
    REGISTRY.lock().sent.iter().any(|m| m.to_pid == pid && m.to_tid == tid && !m.taken)
}

/// Process `pid` replies to sent message `id`: the events to set (the
/// sender's reply event; its queue event for SEND_CALLBACK). A notify
/// message or one whose sender stopped waiting goes.
pub fn reply(
    pid: u32,
    id: u64,
    result: i64,
    out: Vec<u8>,
) -> Result<[Option<ObjectRef>; 2], NtStatus> {
    let mut r = REGISTRY.lock();
    let at = r
        .sent
        .iter()
        .position(|m| m.id == id && m.to_pid == pid && m.taken && m.reply.is_none())
        .ok_or(NtStatus::INVALID_PARAMETER)?;
    if r.sent[at].flags & SEND_NOTIFY != 0 || r.sent[at].abandoned {
        r.sent.remove(at);
        return Ok([None, None]);
    }
    let callback = r.sent[at].flags & SEND_CALLBACK != 0;
    let from = r.sent[at].from_pid;
    let wake = if callback {
        r.listeners.iter().find(|(p, _)| *p == from).map(|(_, e)| e.clone())
    } else {
        None
    };
    let m = &mut r.sent[at];
    m.reply = Some((result, out));
    Ok([Some(m.done.clone()), wake])
}

/// `pid`'s queue event, set when messages are posted to it.
pub fn listen(pid: u32, event: ObjectRef) -> Result<(), NtStatus> {
    let mut r = REGISTRY.lock();
    r.listeners.retain(|(p, _)| *p != pid);
    r.listeners.try_push((pid, event)).map_err(|_| NtStatus::NO_MEMORY)
}

/// Thread `tid` of process `pid` has a message queue (it created a
/// window or called a message function).
pub fn has_queue(pid: u32, tid: u32) -> Result<(), NtStatus> {
    let mut r = REGISTRY.lock();
    if r.queues.iter().any(|&(p, t)| p == pid && t == tid) {
        return Ok(());
    }
    r.queues.try_push((pid, tid)).map_err(|_| NtStatus::NO_MEMORY)
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
        .try_push(Posted { pid, tid: 0, hwnd, message, wparam, lparam })
        .map_err(|_| NtStatus::NO_MEMORY)?;
    Ok(r.listeners.iter().find(|(p, _)| *p == pid).map(|(_, e)| e.clone()))
}

/// Posts a message to thread `tid` (any process's: PostThreadMessage);
/// its process's event to set. STATUS_INVALID_CID when the thread has no
/// message queue (or is none).
pub fn post_thread(
    tid: u32,
    message: u32,
    wparam: u64,
    lparam: u64,
) -> Result<Option<ObjectRef>, NtStatus> {
    let mut r = REGISTRY.lock();
    let pid = r.queues.iter().find(|&&(_, t)| t == tid).ok_or(NtStatus::INVALID_CID)?.0;
    if r.posted.len() >= MAX_POSTED {
        return Err(NtStatus::INSUFFICIENT_RESOURCES);
    }
    r.posted
        .try_push(Posted { pid, tid, hwnd: 0, message, wparam, lparam })
        .map_err(|_| NtStatus::NO_MEMORY)?;
    Ok(r.listeners.iter().find(|(p, _)| *p == pid).map(|(_, e)| e.clone()))
}

/// Takes up to `out.len()` of the messages posted to `pid`'s windows and
/// threads, oldest first; how many.
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

/// The sends `to` matches fail (their waiting senders wake: the events
/// to set go into `wake`); notify messages and those nobody waits for go.
fn fail_sends(r: &mut Registry, to: impl Fn(&Sent) -> bool, wake: &mut Vec<ObjectRef>) {
    r.sent.retain_mut(|m| {
        if !to(m) || m.reply.is_some() {
            return true;
        }
        if m.flags & SEND_NOTIFY != 0 || m.abandoned {
            return false;
        }
        m.failed = true;
        let _ = wake.try_push(m.done.clone());
        true
    });
}

/// The sends `from` matches lose their sender: answered ones go, the
/// others are still handled (a notify message reaches its window after its
/// sender ended, as on Windows) and go when replied.
fn abandon_sends(r: &mut Registry, from: impl Fn(&Sent) -> bool) {
    r.sent.retain_mut(|m| {
        if !from(m) {
            return true;
        }
        m.abandoned = true;
        m.reply.is_none() && !m.failed
    });
}

/// A process ended: its windows go.
pub fn process_ended(pid: u32) {
    let mut wake: Vec<ObjectRef> = Vec::new();
    let mut r = REGISTRY.lock();
    r.windows.retain(|w| w.pid != pid);
    r.posted.retain(|m| m.pid != pid);
    r.listeners.retain(|(p, _)| *p != pid);
    r.queues.retain(|&(p, _)| p != pid);
    // Its own sends go; sends to it fail (their senders wake).
    abandon_sends(&mut r, |m| m.from_pid == pid);
    fail_sends(&mut r, |m| m.to_pid == pid, &mut wake);
    drop(r);
    for e in &wake {
        super::wait::set_signal(e, 1);
    }
}

/// A thread ended: its windows are destroyed with it (as on Windows), its
/// queue goes, and the messages sent to it fail.
pub fn thread_ended(pid: u32, tid: u32) {
    let mut wake: Vec<ObjectRef> = Vec::new();
    let mut r = REGISTRY.lock();
    r.windows.retain(|w| !(w.pid == pid && w.tid == tid));
    r.posted.retain(|m| !(m.pid == pid && m.tid == tid && m.tid != 0));
    r.queues.retain(|&(p, t)| !(p == pid && t == tid));
    abandon_sends(&mut r, |m| m.from_pid == pid && m.from_tid == tid);
    fail_sends(&mut r, |m| m.to_pid == pid && m.to_tid == tid, &mut wake);
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
        let mut got = [Posted::EMPTY; 4];
        assert_eq!(take_posted(902, &mut got), 0);
        assert_eq!(take_posted(901, &mut got), 1);
        assert_eq!(
            got[0],
            Posted { pid: 901, tid: 0, hwnd: b, message: 0x8001, wparam: 7, lparam: 9 }
        );
        assert_eq!(take_posted(901, &mut got), 0);
        // A sent message: handed to the window's thread, replied to.
        let (id, wake) = send(request(902, 1, b, 0x000d, 8, 0)).unwrap();
        assert!(wake.is_none());
        assert!(take_reply(902, id).is_none());
        assert!(receive(901, 76).is_none()); // another thread
        let got = receive(901, 0).unwrap();
        assert_eq!((got.id, got.hwnd, got.message, got.out_max), (id, b, 0x000d, 8));
        assert!(receive(901, 0).is_none());
        let mut out = Vec::new();
        out.try_extend_from_slice(b"hi").unwrap();
        let [done, wake] = reply(901, id, 2, out).unwrap();
        assert!(done.is_some() && wake.is_none());
        let (result, data) = take_reply(902, id).unwrap().unwrap();
        assert_eq!((result, &data[..]), (2, &b"hi"[..]));
        // A receiver that ends fails its senders.
        let (id, _) = send(request(902, 1, b, 0x8001, 0, 0)).unwrap();
        process_ended(901);
        assert_eq!(take_reply(902, id).map(|r| r.is_err()), Some(true));
        assert!(query(top).is_none() && query(b).is_none());
    }

    fn request(
        from_pid: u32,
        from_tid: u32,
        hwnd: u64,
        message: u32,
        out_max: u32,
        flags: u32,
    ) -> Request {
        Request {
            from_pid,
            from_tid,
            hwnd,
            message,
            wparam: 4,
            lparam: 0,
            data: Vec::new(),
            out_max,
            flags,
            now: 1000,
        }
    }

    #[test]
    fn thread_messages_notify_timeouts_and_thread_ends() {
        let w = register(911, Info { tid: 5, ..Info::default() }).unwrap();
        let other = register(911, Info { tid: 9, ..Info::default() }).unwrap();
        // PostThreadMessage needs a thread with a queue.
        assert!(matches!(post_thread(5, 0x8000, 1, 2), Err(NtStatus::INVALID_CID)));
        has_queue(911, 5).unwrap();
        has_queue(911, 5).unwrap();
        post_thread(5, 0x8000, 1, 2).unwrap();
        let mut got = [Posted::EMPTY; 2];
        assert_eq!(take_posted(911, &mut got), 1);
        assert_eq!(
            got[0],
            Posted { pid: 911, tid: 5, hwnd: 0, message: 0x8000, wparam: 1, lparam: 2 }
        );
        // A notify message goes once handled; nobody gets its reply.
        let (id, _) = send(request(912, 1, w, 0x8001, 0, SEND_NOTIFY)).unwrap();
        let got = receive(911, 5).unwrap();
        assert_eq!(got.flags, SEND_NOTIFY);
        assert!(reply(911, id, 1, Vec::new()).unwrap()[0].is_none());
        assert_eq!(take_reply(912, id).map(|r| r.is_err()), Some(true));
        // A timed-out send is still handled; its reply goes nowhere.
        let (id, _) = send(request(912, 1, w, 0x8002, 0, 0)).unwrap();
        abandon(912, id).unwrap();
        assert!(receive(911, 5).is_some());
        assert!(reply(911, id, 1, Vec::new()).unwrap()[0].is_none());
        assert!(sent_event(912, id).is_none());
        // A receiver that leaves a message waiting HUNG_MS is hung.
        let (_, _) = send(request(912, 1, w, 0x8003, 0, 0)).unwrap();
        let mut hung = request(912, 1, w, 0x8004, 0, SEND_ABORT_IF_HUNG);
        hung.now = 1000 + HUNG_MS - 1;
        let (late, _) = send(hung).unwrap();
        let mut hung = request(912, 1, w, 0x8004, 0, SEND_ABORT_IF_HUNG);
        hung.now = 1000 + HUNG_MS;
        assert!(matches!(send(hung), Err(NtStatus::TIMEOUT)));
        // The thread ends: its windows and queue go, sends to it fail;
        // the other thread's window stays.
        thread_ended(911, 5);
        assert!(query(w).is_none() && query(other).is_some());
        assert_eq!(take_reply(912, late).map(|r| r.is_err()), Some(true));
        assert!(matches!(post_thread(5, 0x8000, 1, 2), Err(NtStatus::INVALID_CID)));
        assert!(matches!(send(request(912, 1, w, 0x8001, 0, 0)), Err(NtStatus::INVALID_HANDLE)));
        // A message whose sender thread ended is still handled; its reply
        // goes nowhere.
        let (id, _) = send(request(912, 3, other, 0x8001, 0, SEND_NOTIFY)).unwrap();
        thread_ended(912, 3);
        assert_eq!(receive(911, 9).map(|m| m.id), Some(id));
        assert!(reply(911, id, 0, Vec::new()).unwrap()[0].is_none());
        assert!(sent_event(912, id).is_none());
        process_ended(911);
    }
}
