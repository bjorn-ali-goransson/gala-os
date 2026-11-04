//! The window registry: every window of a session's processes
//! (top-level and child; one registry per session since RS step B2,
//! `Windows` in nt::session::Desk), as Windows' win32k keeps them, so that one process can find
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
use core::sync::atomic::{AtomicU64, Ordering};

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
    /// Version 2: the top-level window's id on shasha's screen (the
    /// `nt::desktop` window user32 made for it; 0: none, a child, or a
    /// version 1 record). GalaDesktopAction names windows by HWND and
    /// finds the screen's window through it.
    pub desktop: u32,
}

pub const CLASS_CHARS: usize = 64;
pub const TEXT_CHARS: usize = 256;
/// A version 1 record's size.
pub const INFO_SIZE_V1: usize = 88 + 2 * CLASS_CHARS + 2 * TEXT_CHARS;
/// A version 2 record's size: version 1, then the screen's window id
/// (u32) and 4 reserved bytes (zero).
pub const INFO_SIZE: usize = INFO_SIZE_V1 + 8;
/// The record's format version (bytes 84..88; ARCHITECTURE invariant 3:
/// everything shared is versioned). A new version adds fields at the end;
/// readers accept it and the previous one (version 1: no screen window).
pub const INFO_VERSION: u32 = 2;
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
            desktop: 0,
        }
    }
}

impl Info {
    /// The record's format version, if `b` holds a whole record of one
    /// this kernel reads (1 or 2).
    pub fn version(b: &[u8]) -> Option<u32> {
        let v = u32::from_le_bytes(b.get(84..88)?.try_into().ok()?);
        let size = match v {
            1 => INFO_SIZE_V1,
            2 => INFO_SIZE,
            _ => return None,
        };
        (b.len() >= size).then_some(v)
    }

    /// Reads a version 1 or 2 record.
    pub fn from_bytes(b: &[u8]) -> Option<Info> {
        let version = Self::version(b)?;
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
        if version >= 2 {
            info.desktop = u32_at(INFO_SIZE_V1);
        }
        Some(info)
    }

    /// Writes the record as format `version` (1, or the current one for
    /// anything else) into `b`; its size.
    pub fn to_bytes_as(&self, version: u32, b: &mut [u8; INFO_SIZE]) -> usize {
        self.to_bytes(b);
        if version == 1 {
            b[84..88].copy_from_slice(&1u32.to_le_bytes());
            b[INFO_SIZE_V1..].fill(0);
            return INFO_SIZE_V1;
        }
        INFO_SIZE
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
        b[INFO_SIZE_V1..INFO_SIZE_V1 + 4].copy_from_slice(&self.desktop.to_le_bytes());
        b[INFO_SIZE_V1 + 4..INFO_SIZE].fill(0);
    }
}

struct Registry {
    windows: Vec<Info>,
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
    /// UIPI's message filters (ChangeWindowMessageFilter(Ex)): messages a
    /// process lets through from lower integrity levels, process-wide or
    /// for one of its windows.
    filters: Vec<Filter>,
}

/// A message process `pid` lets through UIPI's filter: for its window
/// `hwnd`, or process-wide (hwnd 0: ChangeWindowMessageFilter).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Filter {
    pid: u32,
    hwnd: u64,
    message: u32,
}

/// Who sends or posts a message, for UIPI (User Interface Privilege
/// Isolation): the process and its integrity level (its primary token's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sender {
    pub pid: u32,
    pub level: u32,
}

/// ChangeWindowMessageFilterEx's actions (ChangeWindowMessageFilter's
/// MSGFLT_ADD and MSGFLT_REMOVE are ALLOW and DISALLOW).
pub const MSGFLT_RESET: u32 = 0;
pub const MSGFLT_ALLOW: u32 = 1;
pub const MSGFLT_DISALLOW: u32 = 2;
/// CHANGEFILTERSTRUCT's ExtStatus.
pub const MSGFLTINFO_NONE: u32 = 0;
pub const MSGFLTINFO_ALREADYALLOWED_FORWND: u32 = 1;
pub const MSGFLTINFO_ALREADYDISALLOWED_FORWND: u32 = 2;
pub const MSGFLTINFO_ALLOWED_HIGHER: u32 = 3;
/// SECURITY_MANDATORY_LOW_RID: processes at or below it may not change
/// the message filter.
pub const LOW_LEVEL: u32 = 0x1000;

/// The messages UIPI lets through whatever the levels and the filters
/// ("certain messages whose value is smaller than WM_USER are required to
/// pass through the filter"; Microsoft does not list them: this is the
/// list commonly given for Windows, to verify on Windows): WM_NULL,
/// WM_MOVE, WM_SIZE, WM_GETTEXT, WM_GETTEXTLENGTH, WM_GETHOTKEY,
/// WM_GETICON, WM_RENDERFORMAT, WM_DRAWCLIPBOARD, WM_CHANGECBCHAIN,
/// WM_THEMECHANGED.
pub fn always_passes(message: u32) -> bool {
    matches!(
        message,
        0x0000
            | 0x0003
            | 0x0005
            | 0x000d
            | 0x000e
            | 0x0033
            | 0x007f
            | 0x0305
            | 0x0308
            | 0x030d
            | 0x031a
    )
}

/// gala's own messages below WM_USER (user32's GALA_MENU,
/// GALA_SETWINDOWPOS, GALA_SHOWWINDOW: another process's menu walk,
/// SetWindowPos and ShowWindow of the window): UIPI applies to them as to
/// the functions they stand for, and no filter opens them.
fn gala_private(message: u32) -> bool {
    (0x03f0..=0x03ff).contains(&message)
}

/// UIPI: whether a message from `from` reaches process `to_pid` (at
/// integrity level `to_level`), its window `hwnd` (0: a thread message):
/// the same process, a sender of equal or higher level, a message that
/// always passes, or one the receiver's filters let through (for that
/// window or process-wide).
fn passes(r: &Registry, from: Sender, to_pid: u32, to_level: u32, hwnd: u64, message: u32) -> bool {
    if from.pid == to_pid || from.level >= to_level || always_passes(message) {
        return true;
    }
    !gala_private(message)
        && r.filters
            .iter()
            .any(|f| f.pid == to_pid && f.message == message && (f.hwnd == 0 || f.hwnd == hwnd))
}

/// UIPI for what is no message (gala's desktop actions on a window): the
/// caller passes where a message would without the filters (the same
/// process, or an equal or higher integrity level).
pub fn may_act_on(from: Sender, to_pid: u32, to_level: u32) -> bool {
    from.pid == to_pid || from.level >= to_level
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

/// One session's window registry (ROADMAP RS step B2): its windows,
/// message queues, posted and sent messages and UIPI filters, behind its
/// own lock. Each session's desktop (nt::session::Desk) has one; a
/// process reaches only its own session's, so windows, messages and
/// thread queues never cross sessions (another session's HWND is no
/// window: STATUS_INVALID_HANDLE, as Windows' per-session handle tables
/// make it). HWNDs come from one counter for every session (NEXT_HWND),
/// so a value is never reused across sessions.
pub struct Windows(IrqMutex<Registry>);

/// The next HWND's index (all sessions).
static NEXT_HWND: AtomicU64 = AtomicU64::new(0);

impl Default for Windows {
    fn default() -> Self {
        Self::new()
    }
}

impl Windows {
    pub const fn new() -> Windows {
        Windows(IrqMutex::new(Registry {
            windows: Vec::new(),
            posted: Vec::new(),
            listeners: Vec::new(),
            sent: Vec::new(),
            next_sent: 1,
            queues: Vec::new(),
            filters: Vec::new(),
        }))
    }

    /// How much the registry holds (the state dump's line).
    pub fn counts(&self) -> Counts {
        let r = self.0.lock();
        Counts {
            windows: r.windows.len(),
            top_level: r.windows.iter().filter(|w| w.parent == 0).count(),
            queues: r.queues.len(),
            posted: r.posted.len(),
            sent: r.sent.len(),
            filters: r.filters.len(),
        }
    }

    /// Up to `out.len()` of the top-level windows (the state dump); how
    /// many were copied.
    pub fn top_level(&self, out: &mut [Info]) -> usize {
        let r = self.0.lock();
        let mut n = 0;
        for (o, w) in out.iter_mut().zip(r.windows.iter().filter(|w| w.parent == 0)) {
            *o = *w;
            n += 1;
        }
        n
    }

    /// Whether window `hwnd` is in this registry (whose process, if so).
    pub fn holds(&self, hwnd: u64) -> Option<u32> {
        self.0.lock().windows.iter().find(|w| w.hwnd == hwnd).map(|w| w.pid)
    }
}

/// What a session's registry holds (Windows::counts).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub windows: usize,
    pub top_level: usize,
    pub queues: usize,
    pub posted: usize,
    pub sent: usize,
    pub filters: usize,
}

impl Windows {
    /// ChangeWindowMessageFilterEx (`hwnd` nonzero) and
    /// ChangeWindowMessageFilter (`hwnd` 0: process-wide) for process `pid` at
    /// integrity level `level`: the ExtStatus (CHANGEFILTERSTRUCT's table).
    /// STATUS_ACCESS_DENIED for a process at or below Low or another
    /// process's window, STATUS_INVALID_HANDLE for no window,
    /// STATUS_INVALID_PARAMETER for another action. Messages that always
    /// pass (and gala's own) stay as they are, as documented ("no effect").
    pub fn change_filter(
        &self,
        pid: u32,
        level: u32,
        hwnd: u64,
        message: u32,
        action: u32,
    ) -> Result<u32, NtStatus> {
        if action > MSGFLT_DISALLOW {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        if level <= LOW_LEVEL {
            return Err(NtStatus::ACCESS_DENIED);
        }
        let mut r = self.0.lock();
        if hwnd != 0 {
            let owner =
                r.windows.iter().find(|w| w.hwnd == hwnd).ok_or(NtStatus::INVALID_HANDLE)?.pid;
            if owner != pid {
                return Err(NtStatus::ACCESS_DENIED);
            }
        }
        let at = |r: &Registry, h: u64, m: u32| {
            r.filters.iter().position(|f| f.pid == pid && f.hwnd == h && f.message == m)
        };
        if action == MSGFLT_RESET {
            if hwnd != 0 {
                r.filters.retain(|f| !(f.pid == pid && f.hwnd == hwnd));
            }
            return Ok(MSGFLTINFO_NONE);
        }
        let mine = at(&r, hwnd, message);
        // Allowed above the window: process-wide, or always.
        let higher = hwnd != 0 && (at(&r, 0, message).is_some() || always_passes(message));
        if action == MSGFLT_ALLOW {
            if mine.is_some() {
                return Ok(MSGFLTINFO_ALREADYALLOWED_FORWND);
            }
            if !always_passes(message) && !gala_private(message) {
                r.filters
                    .try_push(Filter { pid, hwnd, message })
                    .map_err(|_| NtStatus::NO_MEMORY)?;
            }
            return Ok(MSGFLTINFO_NONE);
        }
        // MSGFLT_DISALLOW.
        let status = match (higher, mine) {
            (true, _) => MSGFLTINFO_ALLOWED_HIGHER,
            (false, Some(_)) => MSGFLTINFO_NONE,
            (false, None) => MSGFLTINFO_ALREADYDISALLOWED_FORWND,
        };
        if let Some(i) = mine {
            r.filters.remove(i);
        }
        Ok(status)
    }

    /// The process of window `hwnd` (UIPI's receiver).
    pub fn owner_pid(&self, hwnd: u64) -> Option<u32> {
        self.0.lock().windows.iter().find(|w| w.hwnd == hwnd).map(|w| w.pid)
    }

    /// The process of thread `tid`'s message queue (PostThreadMessage's
    /// receiver).
    pub fn queue_owner(&self, tid: u32) -> Option<u32> {
        self.0.lock().queues.iter().find(|&&(_, t)| t == tid).map(|&(p, _)| p)
    }

    /// Queues a sent message for its window's thread: its id and the
    /// receiver's queue event to set. STATUS_INVALID_HANDLE for no window,
    /// STATUS_ACCESS_DENIED when UIPI blocks it (`passes`), STATUS_TIMEOUT
    /// for a hung receiver with SEND_ABORT_IF_HUNG.
    pub fn send(&self, req: Request) -> Result<(u64, Option<ObjectRef>), NtStatus> {
        let done = KObject::new(ObjectBody::Event(Event {
            kind: EventType::Notification,
            dispatcher: Dispatcher::new(0),
        }))
        .map_err(|_| NtStatus::NO_MEMORY)?;
        let mut r = self.0.lock();
        let (to_pid, to_tid) = r
            .windows
            .iter()
            .find(|w| w.hwnd == req.hwnd)
            .map(|w| (w.pid, w.tid))
            .ok_or(NtStatus::INVALID_HANDLE)?;
        let from = Sender { pid: req.from_pid, level: req.from_level };
        if !passes(&r, from, to_pid, req.to_level, req.hwnd, req.message) {
            return Err(NtStatus::ACCESS_DENIED);
        }
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
    pub fn sent_event(&self, from_pid: u32, id: u64) -> Option<ObjectRef> {
        let r = self.0.lock();
        r.sent.iter().find(|m| m.id == id && m.from_pid == from_pid).map(|m| m.done.clone())
    }

    /// What became of `from_pid`'s sent message `id`: Some(Ok(result,
    /// output)) once replied (the entry goes), Some(Err) when the receiver
    /// is gone (the entry goes), None while waiting.
    pub fn take_reply(&self, from_pid: u32, id: u64) -> Option<Result<(i64, Vec<u8>), NtStatus>> {
        let mut r = self.0.lock();
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
    pub fn abandon(&self, from_pid: u32, id: u64) -> Result<(), NtStatus> {
        let mut r = self.0.lock();
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

    /// Hands the oldest message sent to thread `tid` of process `pid` to it
    /// (taken: it replies with `reply`).
    pub fn receive(&self, pid: u32, tid: u32) -> Option<Received> {
        let mut r = self.0.lock();
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
    pub fn sent_waiting(&self, pid: u32, tid: u32) -> bool {
        self.0.lock().sent.iter().any(|m| m.to_pid == pid && m.to_tid == tid && !m.taken)
    }

    /// Process `pid` replies to sent message `id`: the events to set (the
    /// sender's reply event; its queue event for SEND_CALLBACK). A notify
    /// message or one whose sender stopped waiting goes.
    pub fn reply(
        &self,
        pid: u32,
        id: u64,
        result: i64,
        out: Vec<u8>,
    ) -> Result<[Option<ObjectRef>; 2], NtStatus> {
        let mut r = self.0.lock();
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
    pub fn listen(&self, pid: u32, event: ObjectRef) -> Result<(), NtStatus> {
        let mut r = self.0.lock();
        r.listeners.retain(|(p, _)| *p != pid);
        r.listeners.try_push((pid, event)).map_err(|_| NtStatus::NO_MEMORY)
    }

    /// `pid`'s queue event (GalaWindowListen), if it has one: WinEvents for
    /// its hooks set it (nt::winevent).
    pub fn listener(&self, pid: u32) -> Option<ObjectRef> {
        self.0.lock().listeners.iter().find(|(p, _)| *p == pid).map(|(_, e)| e.clone())
    }

    /// Thread `tid` of process `pid` has a message queue (it created a
    /// window or called a message function).
    pub fn has_queue(&self, pid: u32, tid: u32) -> Result<(), NtStatus> {
        let mut r = self.0.lock();
        if r.queues.iter().any(|&(p, t)| p == pid && t == tid) {
            return Ok(());
        }
        r.queues.try_push((pid, tid)).map_err(|_| NtStatus::NO_MEMORY)
    }

    /// Posts a message from `from` to window `hwnd` (any process's, at
    /// integrity level `to_level`); the owner's event to set (by the caller,
    /// outside the lock). STATUS_ACCESS_DENIED when UIPI blocks it.
    pub fn post(
        &self,
        from: Sender,
        to_level: u32,
        hwnd: u64,
        message: u32,
        wparam: u64,
        lparam: u64,
    ) -> Result<Option<ObjectRef>, NtStatus> {
        let mut r = self.0.lock();
        let pid = r.windows.iter().find(|w| w.hwnd == hwnd).ok_or(NtStatus::INVALID_HANDLE)?.pid;
        if !passes(&r, from, pid, to_level, hwnd, message) {
            return Err(NtStatus::ACCESS_DENIED);
        }
        if r.posted.len() >= MAX_POSTED {
            return Err(NtStatus::INSUFFICIENT_RESOURCES);
        }
        r.posted
            .try_push(Posted { pid, tid: 0, hwnd, message, wparam, lparam })
            .map_err(|_| NtStatus::NO_MEMORY)?;
        Ok(r.listeners.iter().find(|(p, _)| *p == pid).map(|(_, e)| e.clone()))
    }

    /// Posts a message from `from` to thread `tid` (any process's, at
    /// integrity level `to_level`: PostThreadMessage); its process's event to
    /// set. STATUS_INVALID_CID when the thread has no message queue (or is
    /// none), STATUS_ACCESS_DENIED when UIPI blocks it.
    pub fn post_thread(
        &self,
        from: Sender,
        to_level: u32,
        tid: u32,
        message: u32,
        wparam: u64,
        lparam: u64,
    ) -> Result<Option<ObjectRef>, NtStatus> {
        let mut r = self.0.lock();
        let pid = r.queues.iter().find(|&&(_, t)| t == tid).ok_or(NtStatus::INVALID_CID)?.0;
        if !passes(&r, from, pid, to_level, 0, message) {
            return Err(NtStatus::ACCESS_DENIED);
        }
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
    pub fn take_posted(&self, pid: u32, out: &mut [Posted]) -> usize {
        let mut r = self.0.lock();
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
    pub fn register(&self, pid: u32, mut info: Info) -> Result<u64, NtStatus> {
        let mut r = self.0.lock();
        r.windows.try_reserve(1).map_err(|_| NtStatus::NO_MEMORY)?;
        let hwnd = FIRST_HWND + NEXT_HWND.fetch_add(1, Ordering::Relaxed) * HWND_STEP;
        info.hwnd = hwnd;
        info.pid = pid;
        r.windows.try_push(info).map_err(|_| NtStatus::NO_MEMORY)?;
        Ok(hwnd)
    }

    /// Replaces the record of `pid`'s window `info.hwnd`.
    /// Whether this is the first record with a class (user32 registers
    /// a window before it knows its class and text; the event stream
    /// describes the window then).
    pub fn update(&self, pid: u32, mut info: Info) -> Result<bool, NtStatus> {
        let mut r = self.0.lock();
        let w = r
            .windows
            .iter_mut()
            .find(|w| w.hwnd == info.hwnd && w.pid == pid)
            .ok_or(NtStatus::INVALID_HANDLE)?;
        info.pid = pid;
        let described = w.class_len == 0 && info.class_len > 0;
        *w = info;
        Ok(described)
    }

    /// `pid`'s window `hwnd` is gone.
    pub fn unregister(&self, pid: u32, hwnd: u64) -> Result<(), NtStatus> {
        let mut r = self.0.lock();
        let before = r.windows.len();
        r.windows.retain(|w| !(w.hwnd == hwnd && w.pid == pid));
        if r.windows.len() == before {
            return Err(NtStatus::INVALID_HANDLE);
        }
        r.filters.retain(|f| !(f.pid == pid && f.hwnd == hwnd));
        Ok(())
    }

    /// Any process's window `hwnd`.
    pub fn query(&self, hwnd: u64) -> Option<Info> {
        self.0.lock().windows.iter().find(|w| w.hwnd == hwnd).copied()
    }

    /// The HWND of process `pid`'s top-level window that is window `desktop`
    /// on shasha's screen (GalaDesktopAction's window list).
    pub fn hwnd_of_desktop(&self, pid: u32, desktop: u32) -> Option<u64> {
        if desktop == 0 {
            return None;
        }
        let r = self.0.lock();
        r.windows
            .iter()
            .find(|w| w.pid == pid && w.desktop == desktop && w.parent == 0)
            .map(|w| w.hwnd)
    }

    /// The windows whose parent is `parent` (0: the top-level windows), the
    /// highest first, into `out`; how many there are (more than `out` holds
    /// when it is short).
    pub fn children(&self, parent: u64, out: &mut [u64]) -> usize {
        let r = self.0.lock();
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

    /// The sends `to` matches fail and their waiting senders wake; notify
    /// messages and those nobody waits for go. A thread's or a process's end:
    /// nothing is allocated (a sender left waiting would wait for ever), so the
    /// events are set in rounds of WAKE_BATCH outside the registry's lock.
    fn fail_sends(&self, to: impl Fn(&Sent) -> bool) {
        loop {
            let mut wake: [Option<ObjectRef>; WAKE_BATCH] = Default::default();
            let mut n = 0;
            let mut more = false;
            self.0.lock().sent.retain_mut(|m| {
                if !to(m) || m.reply.is_some() || m.failed {
                    return true;
                }
                if m.flags & SEND_NOTIFY != 0 || m.abandoned {
                    return false;
                }
                if n == WAKE_BATCH {
                    more = true;
                    return true;
                }
                m.failed = true;
                wake[n] = Some(m.done.clone());
                n += 1;
                true
            });
            for e in wake.iter().flatten() {
                super::wait::set_signal(e, 1);
            }
            if !more {
                return;
            }
        }
    }

    /// A process ended: its windows go.
    pub fn process_ended(&self, pid: u32) {
        {
            let mut r = self.0.lock();
            r.windows.retain(|w| w.pid != pid);
            r.posted.retain(|m| m.pid != pid);
            r.listeners.retain(|(p, _)| *p != pid);
            r.queues.retain(|&(p, _)| p != pid);
            r.filters.retain(|f| f.pid != pid);
            // Its own sends go; sends to it fail (their senders wake).
            abandon_sends(&mut r, |m| m.from_pid == pid);
        }
        self.fail_sends(|m| m.to_pid == pid);
    }

    /// A thread ended: its windows are destroyed with it (as on Windows), its
    /// queue goes, and the messages sent to it fail.
    pub fn thread_ended(&self, pid: u32, tid: u32) {
        {
            let mut r = self.0.lock();
            let Registry { windows, filters, .. } = &mut *r;
            filters.retain(|f| {
                f.pid != pid
                    || f.hwnd == 0
                    || windows.iter().any(|w| w.hwnd == f.hwnd && !(w.pid == pid && w.tid == tid))
            });
            r.windows.retain(|w| !(w.pid == pid && w.tid == tid));
            r.posted.retain(|m| !(m.pid == pid && m.tid == tid && m.tid != 0));
            r.queues.retain(|&(p, t)| !(p == pid && t == tid));
            abandon_sends(&mut r, |m| m.from_pid == pid && m.from_tid == tid);
        }
        self.fail_sends(|m| m.to_pid == pid && m.to_tid == tid);
    }
}

/// A message to send (GalaWindowSend): from thread `from_tid` of process
/// `from_pid` to window `hwnd`, its marshaled input and the room for its
/// output; `now` is the caller's clock (ms).
pub struct Request {
    pub from_pid: u32,
    /// The sender's integrity level and the receiver's (UIPI).
    pub from_level: u32,
    pub to_level: u32,
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

/// How many failed sends' senders one round of [`fail_sends`] wakes.
const WAKE_BATCH: usize = 16;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_and_windows_are_found_by_anyone() {
        let reg = Windows::new();
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
        info.desktop = 7;
        let mut bytes = [0u8; INFO_SIZE];
        info.to_bytes(&mut bytes);
        assert_eq!(Info::from_bytes(&bytes), Some(info));
        // A version 1 record (no screen window) is still read, also when
        // only its own size is there.
        assert_eq!(info.to_bytes_as(1, &mut bytes), INFO_SIZE_V1);
        let v1 = Info::from_bytes(&bytes[..INFO_SIZE_V1]).unwrap();
        assert_eq!(v1, Info { desktop: 0, ..info });
        // A version 2 record cut short, and an unknown version, are refused.
        info.to_bytes(&mut bytes);
        assert_eq!(Info::from_bytes(&bytes[..INFO_SIZE_V1]), None);
        bytes[84] = 3;
        assert_eq!(Info::from_bytes(&bytes), None);
        let info = Info { desktop: 0, ..info };

        let top = reg.register(901, info).unwrap();
        assert_eq!(top % HWND_STEP, 0);
        let child_info = Info { parent: top, id: 101, z: 2, ..Info::default() };
        let b = reg.register(901, child_info).unwrap();
        let a = reg.register(901, Info { z: 1, ..child_info }).unwrap();
        assert_ne!(a, b);
        // Another process reads them.
        // The screen's window links to its HWND (top-level windows only).
        let shown = reg.register(901, Info { desktop: 5, ..info }).unwrap();
        assert_eq!(reg.hwnd_of_desktop(901, 5), Some(shown));
        assert_eq!(reg.hwnd_of_desktop(902, 5), None);
        assert_eq!(reg.hwnd_of_desktop(901, 0), None);
        reg.unregister(901, shown).unwrap();
        assert_eq!(reg.hwnd_of_desktop(901, 5), None);
        let q = reg.query(top).unwrap();
        assert_eq!((q.pid, q.tid, q.rect, q.text_len), (901, 77, [10, 20, 310, 220], 2));
        let mut kids = [0u64; 1];
        assert_eq!(reg.children(top, &mut kids), 2);
        assert_eq!(kids[0], a); // the higher (smaller z) first
        // Only the owner changes or removes them.
        let moved = Info { rect: [0, 0, 5, 5], ..q };
        assert_eq!(reg.update(902, moved), Err(NtStatus::INVALID_HANDLE));
        reg.update(901, moved).unwrap();
        assert_eq!(reg.query(top).unwrap().rect, [0, 0, 5, 5]);
        assert_eq!(reg.unregister(902, a), Err(NtStatus::INVALID_HANDLE));
        reg.unregister(901, a).unwrap();
        assert_eq!(reg.children(top, &mut kids), 1);
        // A message posted to its window waits for its owner.
        assert!(
            reg.post(Sender { pid: 902, level: 0x2000 }, 0x2000, b, 0x8001, 7, 9)
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            reg.post(Sender { pid: 902, level: 0x2000 }, 0x2000, a, 0x8001, 0, 0),
            Err(NtStatus::INVALID_HANDLE)
        ));
        let mut got = [Posted::EMPTY; 4];
        assert_eq!(reg.take_posted(902, &mut got), 0);
        assert_eq!(reg.take_posted(901, &mut got), 1);
        assert_eq!(
            got[0],
            Posted { pid: 901, tid: 0, hwnd: b, message: 0x8001, wparam: 7, lparam: 9 }
        );
        assert_eq!(reg.take_posted(901, &mut got), 0);
        // A sent message: handed to the window's thread, replied to.
        let (id, wake) = reg.send(request(902, 1, b, 0x000d, 8, 0)).unwrap();
        assert!(wake.is_none());
        assert!(reg.take_reply(902, id).is_none());
        assert!(reg.receive(901, 76).is_none()); // another thread
        let got = reg.receive(901, 0).unwrap();
        assert_eq!((got.id, got.hwnd, got.message, got.out_max), (id, b, 0x000d, 8));
        assert!(reg.receive(901, 0).is_none());
        let mut out = Vec::new();
        out.try_extend_from_slice(b"hi").unwrap();
        let [done, wake] = reg.reply(901, id, 2, out).unwrap();
        assert!(done.is_some() && wake.is_none());
        let (result, data) = reg.take_reply(902, id).unwrap().unwrap();
        assert_eq!((result, &data[..]), (2, &b"hi"[..]));
        // A receiver that ends fails its senders.
        let (id, _) = reg.send(request(902, 1, b, 0x8001, 0, 0)).unwrap();
        reg.process_ended(901);
        assert_eq!(reg.take_reply(902, id).map(|r| r.is_err()), Some(true));
        assert!(reg.query(top).is_none() && reg.query(b).is_none());
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
            from_level: 0x2000,
            to_level: 0x2000,
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
        let reg = Windows::new();
        let w = reg.register(911, Info { tid: 5, ..Info::default() }).unwrap();
        let other = reg.register(911, Info { tid: 9, ..Info::default() }).unwrap();
        // PostThreadMessage needs a thread with a queue.
        assert!(matches!(
            reg.post_thread(Sender { pid: 912, level: 0x2000 }, 0x2000, 5, 0x8000, 1, 2),
            Err(NtStatus::INVALID_CID)
        ));
        reg.has_queue(911, 5).unwrap();
        reg.has_queue(911, 5).unwrap();
        reg.post_thread(Sender { pid: 912, level: 0x2000 }, 0x2000, 5, 0x8000, 1, 2).unwrap();
        let mut got = [Posted::EMPTY; 2];
        assert_eq!(reg.take_posted(911, &mut got), 1);
        assert_eq!(
            got[0],
            Posted { pid: 911, tid: 5, hwnd: 0, message: 0x8000, wparam: 1, lparam: 2 }
        );
        // A notify message goes once handled; nobody gets its reply.
        let (id, _) = reg.send(request(912, 1, w, 0x8001, 0, SEND_NOTIFY)).unwrap();
        let got = reg.receive(911, 5).unwrap();
        assert_eq!(got.flags, SEND_NOTIFY);
        assert!(reg.reply(911, id, 1, Vec::new()).unwrap()[0].is_none());
        assert_eq!(reg.take_reply(912, id).map(|r| r.is_err()), Some(true));
        // A timed-out send is still handled; its reply goes nowhere.
        let (id, _) = reg.send(request(912, 1, w, 0x8002, 0, 0)).unwrap();
        reg.abandon(912, id).unwrap();
        assert!(reg.receive(911, 5).is_some());
        assert!(reg.reply(911, id, 1, Vec::new()).unwrap()[0].is_none());
        assert!(reg.sent_event(912, id).is_none());
        // A receiver that leaves a message waiting HUNG_MS is hung.
        let (_, _) = reg.send(request(912, 1, w, 0x8003, 0, 0)).unwrap();
        let mut hung = request(912, 1, w, 0x8004, 0, SEND_ABORT_IF_HUNG);
        hung.now = 1000 + HUNG_MS - 1;
        let (late, _) = reg.send(hung).unwrap();
        let mut hung = request(912, 1, w, 0x8004, 0, SEND_ABORT_IF_HUNG);
        hung.now = 1000 + HUNG_MS;
        assert!(matches!(reg.send(hung), Err(NtStatus::TIMEOUT)));
        // The thread ends: its windows and queue go, sends to it fail;
        // the other thread's window stays.
        reg.thread_ended(911, 5);
        assert!(reg.query(w).is_none() && reg.query(other).is_some());
        assert_eq!(reg.take_reply(912, late).map(|r| r.is_err()), Some(true));
        assert!(matches!(
            reg.post_thread(Sender { pid: 912, level: 0x2000 }, 0x2000, 5, 0x8000, 1, 2),
            Err(NtStatus::INVALID_CID)
        ));
        assert!(matches!(
            reg.send(request(912, 1, w, 0x8001, 0, 0)),
            Err(NtStatus::INVALID_HANDLE)
        ));
        // A message whose sender thread ended is still handled; its reply
        // goes nowhere.
        let (id, _) = reg.send(request(912, 3, other, 0x8001, 0, SEND_NOTIFY)).unwrap();
        reg.thread_ended(912, 3);
        assert_eq!(reg.receive(911, 9).map(|m| m.id), Some(id));
        assert!(reg.reply(911, id, 0, Vec::new()).unwrap()[0].is_none());
        assert!(reg.sent_event(912, id).is_none());
        reg.process_ended(911);
    }

    /// UIPI: a lower integrity level reaches a higher one's windows only
    /// with the messages that always pass and those its filters allow
    /// (ChangeWindowMessageFilterEx, ChangeWindowMessageFilter's table of
    /// ExtStatus values).
    #[test]
    fn uipi_blocks_lower_senders_but_what_filters_allow() {
        let reg = Windows::new();
        const MEDIUM: u32 = 0x2000;
        const WM_APP: u32 = 0x8000;
        let w = reg.register(931, Info { tid: 6, ..Info::default() }).unwrap();
        let v = reg.register(931, Info { tid: 6, ..Info::default() }).unwrap();
        reg.has_queue(931, 6).unwrap();
        let low = Sender { pid: 932, level: LOW_LEVEL };
        let high = Sender { pid: 933, level: 0x3000 };
        let denied =
            |r: Result<Option<ObjectRef>, NtStatus>| matches!(r, Err(NtStatus::ACCESS_DENIED));
        // Lower: blocked, but WM_NULL and WM_GETTEXT; equal or higher, or
        // the same process: everything.
        assert!(denied(reg.post(low, MEDIUM, w, WM_APP, 0, 0)));
        assert!(denied(reg.post_thread(low, MEDIUM, 6, WM_APP, 0, 0)));
        assert!(reg.post(low, MEDIUM, w, 0, 0, 0).is_ok());
        assert!(reg.post(high, MEDIUM, w, WM_APP, 0, 0).is_ok());
        assert!(reg.post(Sender { pid: 931, level: 0 }, MEDIUM, w, WM_APP, 0, 0).is_ok());
        let mut req = request(932, 1, w, WM_APP, 0, 0);
        req.from_level = LOW_LEVEL;
        req.to_level = MEDIUM;
        assert!(matches!(reg.send(req), Err(NtStatus::ACCESS_DENIED)));
        let mut req = request(932, 1, w, 0x000d, 8, 0);
        req.from_level = LOW_LEVEL;
        req.to_level = MEDIUM;
        assert!(reg.send(req).is_ok());
        // gala's SetWindowPos message is never let through.
        assert!(denied(reg.post(low, MEDIUM, w, 0x03f1, 0, 0)));
        // A window's filter: that window only; the ExtStatus values.
        let ex = |h, m, a| reg.change_filter(931, MEDIUM, h, m, a);
        assert_eq!(ex(w, WM_APP, MSGFLT_ALLOW), Ok(MSGFLTINFO_NONE));
        assert_eq!(ex(w, WM_APP, MSGFLT_ALLOW), Ok(MSGFLTINFO_ALREADYALLOWED_FORWND));
        assert!(reg.post(low, MEDIUM, w, WM_APP, 0, 0).is_ok());
        assert!(denied(reg.post(low, MEDIUM, v, WM_APP, 0, 0)));
        assert!(denied(reg.post_thread(low, MEDIUM, 6, WM_APP, 0, 0)));
        assert_eq!(ex(w, WM_APP, MSGFLT_DISALLOW), Ok(MSGFLTINFO_NONE));
        assert_eq!(ex(w, WM_APP, MSGFLT_DISALLOW), Ok(MSGFLTINFO_ALREADYDISALLOWED_FORWND));
        assert!(denied(reg.post(low, MEDIUM, w, WM_APP, 0, 0)));
        assert_eq!(ex(w, 0x03f1, MSGFLT_ALLOW), Ok(MSGFLTINFO_NONE));
        assert!(denied(reg.post(low, MEDIUM, w, 0x03f1, 0, 0)));
        // Process-wide (ChangeWindowMessageFilter): every window and the
        // threads; a window cannot take it back (ALLOWED_HIGHER).
        assert_eq!(ex(0, WM_APP + 1, MSGFLT_ALLOW), Ok(MSGFLTINFO_NONE));
        assert!(reg.post(low, MEDIUM, v, WM_APP + 1, 0, 0).is_ok());
        assert!(reg.post_thread(low, MEDIUM, 6, WM_APP + 1, 0, 0).is_ok());
        assert_eq!(ex(v, WM_APP + 1, MSGFLT_DISALLOW), Ok(MSGFLTINFO_ALLOWED_HIGHER));
        assert!(reg.post(low, MEDIUM, v, WM_APP + 1, 0, 0).is_ok());
        assert_eq!(ex(v, 0x000d, MSGFLT_DISALLOW), Ok(MSGFLTINFO_ALLOWED_HIGHER));
        // MSGFLT_RESET drops the window's own filter.
        assert_eq!(ex(w, WM_APP + 2, MSGFLT_ALLOW), Ok(MSGFLTINFO_NONE));
        assert_eq!(ex(w, 0, MSGFLT_RESET), Ok(MSGFLTINFO_NONE));
        assert!(denied(reg.post(low, MEDIUM, w, WM_APP + 2, 0, 0)));
        // Low (or lower) may not change filters; nor another process's
        // window; a window must exist; actions are 0..=2.
        assert_eq!(reg.change_filter(931, LOW_LEVEL, w, WM_APP, 1), Err(NtStatus::ACCESS_DENIED));
        assert_eq!(reg.change_filter(932, MEDIUM, w, WM_APP, 1), Err(NtStatus::ACCESS_DENIED));
        assert_eq!(reg.change_filter(931, MEDIUM, 0x10, WM_APP, 1), Err(NtStatus::INVALID_HANDLE));
        assert_eq!(reg.change_filter(931, MEDIUM, w, WM_APP, 3), Err(NtStatus::INVALID_PARAMETER));
        // The desktop's actions: no filters, levels only.
        assert!(!may_act_on(low, 931, MEDIUM));
        assert!(may_act_on(high, 931, MEDIUM));
        assert!(may_act_on(Sender { pid: 931, level: 0 }, 931, MEDIUM));
        assert_eq!((reg.owner_pid(w), reg.queue_owner(6)), (Some(931), Some(931)));
        // The process's end drops its filters with its windows.
        reg.process_ended(931);
        assert!(reg.0.lock().filters.iter().all(|f| f.pid != 931));
    }

    /// More waiting senders than one wake round: every one wakes (the
    /// events are set in rounds, nothing is allocated on the way).
    #[test]
    fn a_thread_end_wakes_every_waiting_sender() {
        let reg = Windows::new();
        let w = reg.register(921, Info { tid: 4, ..Info::default() }).unwrap();
        let mut waiting = Vec::new();
        for _ in 0..3 * WAKE_BATCH + 1 {
            let (id, _) = reg.send(request(922, 1, w, 0x8001, 0, 0)).unwrap();
            waiting.push((id, reg.sent_event(922, id).unwrap()));
        }
        reg.thread_ended(921, 4);
        for (id, done) in &waiting {
            assert_eq!(done.dispatcher().map(|d| d.signal_state()), Some(1));
            assert_eq!(reg.take_reply(922, *id).map(|r| r.is_err()), Some(true));
        }
        reg.process_ended(921);
    }
}
