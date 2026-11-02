//! Windows across processes: the kernel's window registry (nt::window),
//! which holds every window of the desktop. user32 registers each window
//! it creates there (the HWND comes from the kernel, so HWNDs are unique
//! across processes), mirrors the fields other processes may ask about
//! (`sync`), and answers questions about other processes' windows from
//! it (`query`, `children`). See docs/ARCHITECTURE.md, "Plan: windows
//! across processes".

use crate::window::{Hwnd, MAX_WINDOWS, window, with};
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaWindowRegister(info: *const u8, hwnd: *mut u64) -> u32;
    fn GalaWindowUpdate(info: *const u8) -> u32;
    fn GalaWindowUnregister(hwnd: u64) -> u32;
    fn GalaWindowQuery(hwnd: u64, info: *mut u8) -> u32;
    fn GalaWindowChildren(parent: u64, hwnds: *mut u64, max: u32, count: *mut u32) -> u32;
    fn GalaWindowPost(hwnd: u64, message: u32, wparam: u64, lparam: u64) -> u32;
    fn GalaWindowPosted(records: *mut u8, max: u32, count: *mut u32) -> u32;
    fn GalaWindowSend(request: *const u8, id: *mut u64) -> u32;
    fn GalaWindowSendWait(id: u64, ms: u32, result: *mut i64, out: *mut u8, len: *mut u32) -> u32;
    fn GalaWindowReceive(tid: u32, head: *mut u8, input: *mut u8, max: u32) -> u32;
    fn GalaWindowReply(id: u64, result: i64, out: *const u8, len: u32) -> u32;
    fn GalaWindowPostThread(tid: u32, message: u32, wparam: u64, lparam: u64) -> u32;
    fn GalaWindowSendAbandon(id: u64) -> u32;
    fn GalaWindowFilter(hwnd: u64, message: u32, action: u32, ext: *mut u32) -> u32;
}

/// The kernel's answer when UIPI blocks a message (a lower integrity
/// level than the receiver's), and the error Windows documents for it.
const STATUS_ACCESS_DENIED: u32 = 0xc000_0022;
const ERROR_ACCESS_DENIED: u32 = 5;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn VirtualAlloc(
        at: *mut core::ffi::c_void,
        size: usize,
        kind: u32,
        protect: u32,
    ) -> *mut core::ffi::c_void;
    fn VirtualFree(at: *mut core::ffi::c_void, size: usize, kind: u32) -> i32;
    fn GetTickCount() -> u32;
}

const CLASS_CHARS: usize = 64;
const TEXT_CHARS: usize = 256;
/// The record's size (nt::window::INFO_SIZE): version 1's, then the
/// window's id on the screen (version 2) and 4 reserved bytes.
const INFO_SIZE: usize = 88 + 2 * CLASS_CHARS + 2 * TEXT_CHARS + 8;
/// The record's format version (nt::window::INFO_VERSION). Records read
/// back (GalaWindowQuery, zeroed buffer) come as version 1.
const INFO_VERSION: u32 = 2;
const FLAG_VISIBLE: u32 = 1;
const FLAG_ENABLED: u32 = 2;
/// Maximized (IsZoomed) and minimized (IsIconic), for UI Automation's
/// Window pattern in other processes.
const FLAG_ZOOMED: u32 = 4;
const FLAG_ICONIC: u32 = 8;

/// Each window slot's HWND (0: free), read without user32's lock.
static HWNDS: [AtomicUsize; MAX_WINDOWS] = [const { AtomicUsize::new(0) }; MAX_WINDOWS];

/// The HWNDs user32 numbers itself when the kernel has no registry (an
/// older kernel): below the registry's (0x20000 up).
const LOCAL_BASE: usize = 0x0001_0000;

pub(crate) fn hwnd_of(i: usize) -> Hwnd {
    HWNDS.get(i).map_or(0, |h| h.load(Ordering::Acquire))
}

pub(crate) fn index_of(hwnd: Hwnd) -> Option<usize> {
    if hwnd == 0 {
        return None;
    }
    HWNDS.iter().position(|h| h.load(Ordering::Acquire) == hwnd)
}

/// A window's record (the registry's).
#[derive(Clone, Copy)]
pub(crate) struct Info {
    pub hwnd: Hwnd,
    pub parent: Hwnd,
    pub owner: Hwnd,
    pub id: usize,
    pub pid: u32,
    pub tid: u32,
    pub style: u32,
    pub ex_style: u32,
    pub rect: [i32; 4],
    pub visible: bool,
    pub enabled: bool,
    pub zoomed: bool,
    pub iconic: bool,
    pub z: i64,
    pub class: [u16; CLASS_CHARS],
    pub class_len: usize,
    pub text: [u16; TEXT_CHARS],
    pub text_len: usize,
    /// The window's id on the screen (GalaWindowCreate's; version 2).
    pub desktop: u32,
}

impl Info {
    fn bytes(&self) -> [u8; INFO_SIZE] {
        let mut b = [0u8; INFO_SIZE];
        b[0..8].copy_from_slice(&(self.hwnd as u64).to_le_bytes());
        b[8..16].copy_from_slice(&(self.parent as u64).to_le_bytes());
        b[16..24].copy_from_slice(&(self.owner as u64).to_le_bytes());
        b[24..32].copy_from_slice(&(self.id as u64).to_le_bytes());
        b[36..40].copy_from_slice(&self.tid.to_le_bytes());
        b[40..44].copy_from_slice(&self.style.to_le_bytes());
        b[44..48].copy_from_slice(&self.ex_style.to_le_bytes());
        for (k, v) in self.rect.iter().enumerate() {
            b[48 + 4 * k..52 + 4 * k].copy_from_slice(&v.to_le_bytes());
        }
        let flags = if self.visible { FLAG_VISIBLE } else { 0 }
            | if self.enabled { FLAG_ENABLED } else { 0 }
            | if self.zoomed { FLAG_ZOOMED } else { 0 }
            | if self.iconic { FLAG_ICONIC } else { 0 };
        b[64..68].copy_from_slice(&flags.to_le_bytes());
        b[68..72].copy_from_slice(&(self.class_len as u32).to_le_bytes());
        b[72..80].copy_from_slice(&self.z.to_le_bytes());
        b[80..84].copy_from_slice(&(self.text_len as u32).to_le_bytes());
        b[84..88].copy_from_slice(&INFO_VERSION.to_le_bytes());
        for (k, c) in self.class.iter().enumerate() {
            b[88 + 2 * k..90 + 2 * k].copy_from_slice(&c.to_le_bytes());
        }
        let t = 88 + 2 * CLASS_CHARS;
        for (k, c) in self.text.iter().enumerate() {
            b[t + 2 * k..t + 2 + 2 * k].copy_from_slice(&c.to_le_bytes());
        }
        let d = t + 2 * TEXT_CHARS;
        b[d..d + 4].copy_from_slice(&self.desktop.to_le_bytes());
        b
    }

    fn from_bytes(b: &[u8; INFO_SIZE]) -> Info {
        let u64_at = |at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap_or([0; 8]));
        let u32_at = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap_or([0; 4]));
        let mut info = Info {
            hwnd: u64_at(0) as Hwnd,
            parent: u64_at(8) as Hwnd,
            owner: u64_at(16) as Hwnd,
            id: u64_at(24) as usize,
            pid: u32_at(32),
            tid: u32_at(36),
            style: u32_at(40),
            ex_style: u32_at(44),
            rect: [u32_at(48) as i32, u32_at(52) as i32, u32_at(56) as i32, u32_at(60) as i32],
            visible: u32_at(64) & FLAG_VISIBLE != 0,
            enabled: u32_at(64) & FLAG_ENABLED != 0,
            zoomed: u32_at(64) & FLAG_ZOOMED != 0,
            iconic: u32_at(64) & FLAG_ICONIC != 0,
            class_len: (u32_at(68) as usize).min(CLASS_CHARS),
            z: u64_at(72) as i64,
            text_len: (u32_at(80) as usize).min(TEXT_CHARS),
            class: [0; CLASS_CHARS],
            text: [0; TEXT_CHARS],
            desktop: 0,
        };
        for (k, c) in info.class.iter_mut().enumerate() {
            *c = u16::from_le_bytes([b[88 + 2 * k], b[89 + 2 * k]]);
        }
        let t = 88 + 2 * CLASS_CHARS;
        for (k, c) in info.text.iter_mut().enumerate() {
            *c = u16::from_le_bytes([b[t + 2 * k], b[t + 1 + 2 * k]]);
        }
        info
    }

    pub(crate) fn class(&self) -> &[u16] {
        &self.class[..self.class_len]
    }

    pub(crate) fn text(&self) -> &[u16] {
        &self.text[..self.text_len]
    }
}

/// The record of our window slot `i` as it is now (None: not a window).
fn local_info(i: usize) -> Option<Info> {
    let hwnd = hwnd_of(i);
    let w = window(hwnd)?;
    let spelled = with(|s| s.classes[w.class].spelled);
    let mut info = Info {
        hwnd,
        parent: w.parent,
        owner: w.owner,
        id: w.id,
        pid: 0,
        tid: w.thread,
        style: w.style,
        ex_style: w.ex_style,
        rect: [0; 4],
        visible: w.visible,
        enabled: !w.disabled,
        zoomed: w.zoomed,
        iconic: w.iconic,
        z: w.z,
        class: [0; CLASS_CHARS],
        class_len: 0,
        text: [0; TEXT_CHARS],
        text_len: 0,
        desktop: 0,
    };
    info.class_len = spelled.iter().position(|&c| c == 0).unwrap_or(spelled.len());
    info.class[..info.class_len].copy_from_slice(&spelled[..info.class_len]);
    info.text_len = w.title.iter().position(|&c| c == 0).unwrap_or(w.title.len());
    info.text[..info.text_len].copy_from_slice(&w.title[..info.text_len]);
    if w.parent == 0 {
        info.desktop = w.desktop;
    }
    crate::tree::GetWindowRect(hwnd, &mut info.rect);
    Some(info)
}

/// Gives the new window in slot `i` its HWND: registered with the
/// kernel, or numbered here without a registry.
pub(crate) fn assign(i: usize) -> Hwnd {
    // Registered with an empty record first (the slot needs its HWND
    // before the record can be read), then filled in.
    let blank = Info {
        hwnd: 0,
        parent: 0,
        owner: 0,
        id: 0,
        pid: 0,
        tid: 0,
        style: 0,
        ex_style: 0,
        rect: [0; 4],
        visible: false,
        enabled: true,
        zoomed: false,
        iconic: false,
        z: 0,
        class: [0; CLASS_CHARS],
        class_len: 0,
        text: [0; TEXT_CHARS],
        text_len: 0,
        desktop: 0,
    };
    let mut hwnd = 0u64;
    // SAFETY: our record and output.
    let status = unsafe { GalaWindowRegister(blank.bytes().as_ptr(), &mut hwnd) };
    let hwnd = if status == 0 && hwnd != 0 { hwnd as Hwnd } else { LOCAL_BASE + i * 0x10 };
    HWNDS[i].store(hwnd, Ordering::Release);
    sync(hwnd);
    hwnd
}

/// Window slot `i` is free again.
pub(crate) fn release(i: usize) {
    let hwnd = HWNDS[i].swap(0, Ordering::AcqRel);
    if hwnd >= 0x2_0000 {
        // SAFETY: a plain call.
        unsafe { GalaWindowUnregister(hwnd as u64) };
    }
}

/// Mirrors our window `hwnd`'s record into the registry.
pub(crate) fn sync(hwnd: Hwnd) {
    let Some(i) = index_of(hwnd) else { return };
    if let Some(info) = local_info(i) {
        // SAFETY: our record.
        unsafe { GalaWindowUpdate(info.bytes().as_ptr()) };
    }
}

/// `sync` for `hwnd` and its descendants (their screen rectangles follow
/// a move).
pub(crate) fn sync_tree(hwnd: Hwnd) {
    let mut all = [0usize; 64];
    let n = crate::tree::subtree(hwnd, &mut all);
    for &h in &all[..n] {
        sync(h);
    }
}

/// Any process's window `hwnd` (ours too).
pub(crate) fn query(hwnd: Hwnd) -> Option<Info> {
    if hwnd == 0 {
        return None;
    }
    let mut b = [0u8; INFO_SIZE];
    // SAFETY: our buffer of INFO_SIZE bytes.
    let status = unsafe { GalaWindowQuery(hwnd as u64, b.as_mut_ptr()) };
    (status == 0).then(|| Info::from_bytes(&b))
}

/// The windows whose parent is `parent` (0: every top-level window of
/// the desktop), highest first, into `out`; how many.
pub(crate) fn children(parent: Hwnd, out: &mut [Hwnd; 256]) -> usize {
    let mut raw = [0u64; 256];
    let mut count = 0u32;
    // SAFETY: our array of 256.
    let status = unsafe { GalaWindowChildren(parent as u64, raw.as_mut_ptr(), 256, &mut count) };
    if status != 0 {
        return 0;
    }
    let n = (count as usize).min(256);
    for (o, &r) in out.iter_mut().zip(&raw[..n]) {
        *o = r as Hwnd;
    }
    n
}

/// Whether `a` (UTF-16, any case for ASCII letters) is `b` (a
/// NUL-terminated string).
pub(crate) fn same_name(a: &[u16], b: *const u16, ignore_case: bool) -> bool {
    let fold = |c: u16| {
        if ignore_case && (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }
    };
    for (k, &c) in a.iter().enumerate() {
        // SAFETY: the caller's NUL-terminated string, read up to its NUL.
        let t = unsafe { *b.add(k) };
        if t == 0 || fold(t) != fold(c) {
            return false;
        }
    }
    // SAFETY: as above: the character after the match.
    unsafe { *b.add(a.len()) == 0 }
}

/// Another process's window `hwnd` (None for ours and for none).
pub(crate) fn remote(hwnd: Hwnd) -> Option<Info> {
    if index_of(hwnd).is_some() || hwnd == crate::tree::DESKTOP {
        return None;
    }
    query(hwnd)
}

/// Whether another process's window and its ancestors are visible
/// (IsWindowVisible).
pub(crate) fn remote_shown(info: &Info) -> bool {
    let mut cur = *info;
    for _ in 0..64 {
        if !cur.visible {
            return false;
        }
        if cur.parent == 0 {
            return true;
        }
        match query(cur.parent) {
            Some(p) => cur = p,
            None => return true,
        }
    }
    true
}

/// Copies `text` into the caller's buffer of `n` characters (truncated,
/// NUL-terminated); the characters copied.
pub(crate) fn copy_out(text: &[u16], buf: *mut u16, n: i32) -> i32 {
    if buf.is_null() || n <= 0 {
        return 0;
    }
    let len = text.len().min(n as usize - 1);
    // SAFETY: the caller's buffer of `n` characters.
    unsafe {
        core::ptr::copy_nonoverlapping(text.as_ptr(), buf, len);
        *buf.add(len) = 0;
    }
    len as i32
}

type EnumProc = extern "system" fn(Hwnd, isize) -> i32;

/// EnumWindows over the desktop's top-level windows (every process's),
/// highest first; None when the registry has none (an older kernel).
pub(crate) fn enum_top_levels(p: EnumProc, data: isize) -> Option<i32> {
    let mut all = [0; 256];
    let n = children(0, &mut all);
    if n == 0 {
        return None;
    }
    Some(i32::from(all[..n].iter().all(|&h| p(h, data) != 0)))
}

/// The first window after `after` (0: from the first) among `parent`'s
/// children (0: the top-level windows of every process) whose class
/// (any case; NULL: any) and text (NULL: any) match.
pub(crate) fn find(parent: Hwnd, after: Hwnd, class: *const u16, title: *const u16) -> Hwnd {
    let mut all = [0; 256];
    let n = children(parent, &mut all);
    let start = if after == 0 {
        0
    } else {
        match all[..n].iter().position(|&h| h == after) {
            Some(k) => k + 1,
            None => return 0,
        }
    };
    let atom = !class.is_null() && (class as usize) <= 0xffff;
    for &h in &all[start..n] {
        let Some(info) = query(h) else { continue };
        if atom {
            // A class atom: only our classes have atoms we know.
            if crate::window::class_atom_of(h) != Some(class as usize as u16) {
                continue;
            }
        } else if !class.is_null() && !same_name(info.class(), class, true) {
            continue;
        }
        if !title.is_null() && !same_name(info.text(), title, false) {
            continue;
        }
        return h;
    }
    0
}

/// The system messages below WM_USER whose parameters carry pointers:
/// they cannot be posted to another process (ERROR_MESSAGE_SYNC_ONLY), as
/// Windows documents for the asynchronous message functions.
pub(crate) fn carries_pointer(msg: u32) -> bool {
    matches!(
        msg,
        0x0001 // WM_CREATE
            | 0x000c // WM_SETTEXT
            | 0x000d // WM_GETTEXT
            | 0x001a // WM_SETTINGCHANGE
            | 0x0024 // WM_GETMINMAXINFO
            | 0x002b
            ..=0x002d // WM_DRAWITEM, MEASUREITEM, DELETEITEM
            | 0x0039 // WM_COMPAREITEM
            | 0x0046 | 0x0047 // WM_WINDOWPOSCHANGING/CHANGED
            | 0x004a // WM_COPYDATA
            | 0x004e // WM_NOTIFY
            | 0x0053 // WM_HELP
            | 0x0081 // WM_NCCREATE
            | 0x0083 // WM_NCCALCSIZE
    ) || in_string(msg)
        || msg == crate::track::GALA_MENU
        || msg == crate::tree::GALA_SETWINDOWPOS
        || matches!(msg, LB_GETTEXT | LB_GETITEMRECT | LB_GETSELITEMS | CB_GETLBTEXT)
}

use crate::listbox::{
    LB_ADDSTRING, LB_FINDSTRING, LB_FINDSTRINGEXACT, LB_GETITEMRECT, LB_GETSELITEMS, LB_GETTEXT,
    LB_GETTEXTLEN, LB_INSERTSTRING, LB_SELECTSTRING,
};
const CB_ADDSTRING: u32 = 0x0143;
const CB_GETLBTEXT: u32 = 0x0148;
const CB_GETLBTEXTLEN: u32 = 0x0149;
const CB_INSERTSTRING: u32 = 0x014a;
const CB_FINDSTRING: u32 = 0x014c;
const CB_SELECTSTRING: u32 = 0x014d;
const CB_FINDSTRINGEXACT: u32 = 0x0158;

/// The messages whose lParam is a string going in (marshaled across
/// processes, as Windows marshals the system messages below WM_USER).
fn in_string(msg: u32) -> bool {
    matches!(
        msg,
        WM_SETTEXT
            | LB_ADDSTRING
            | LB_INSERTSTRING
            | LB_FINDSTRING
            | LB_FINDSTRINGEXACT
            | LB_SELECTSTRING
            | CB_ADDSTRING
            | CB_INSERTSTRING
            | CB_FINDSTRING
            | CB_SELECTSTRING
            | CB_FINDSTRINGEXACT
    )
}

/// PostMessage to another process's window `hwnd` (through the kernel).
pub(crate) fn post_remote(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> i32 {
    const ERROR_INVALID_WINDOW_HANDLE: u32 = 1400;
    const ERROR_MESSAGE_SYNC_ONLY: u32 = 1159;
    if remote(hwnd).is_none() {
        crate::window::set_last_error(ERROR_INVALID_WINDOW_HANDLE);
        return 0;
    }
    if carries_pointer(msg) {
        crate::window::set_last_error(ERROR_MESSAGE_SYNC_ONLY);
        return 0;
    }
    // SAFETY: a plain call.
    let status = unsafe { GalaWindowPost(hwnd as u64, msg, wparam as u64, lparam as u64) };
    if status != 0 {
        // "When a message is blocked by UIPI the last error is set to 5
        // (access denied)."
        crate::window::set_last_error(if status == STATUS_ACCESS_DENIED {
            ERROR_ACCESS_DENIED
        } else {
            ERROR_INVALID_WINDOW_HANDLE
        });
        return 0;
    }
    1
}

/// Moves the messages other processes posted to our windows and threads
/// (and the thread messages we posted ourselves, which go through the
/// kernel too) into the queues of their threads.
pub(crate) fn take_posted() {
    let mut raw = [0u8; 32 * 32];
    loop {
        let mut n = 0u32;
        // SAFETY: our buffer of 32 records.
        if unsafe { GalaWindowPosted(raw.as_mut_ptr(), 32, &mut n) } != 0 || n == 0 {
            return;
        }
        for r in raw.chunks_exact(32).take(n as usize) {
            let u64_at = |at: usize| u64::from_le_bytes(r[at..at + 8].try_into().unwrap_or([0; 8]));
            let u32_at = |at: usize| u32::from_le_bytes(r[at..at + 4].try_into().unwrap_or([0; 4]));
            let hwnd = u64_at(0) as Hwnd;
            let (msg, tid) = (u32_at(8), u32_at(12));
            let (wparam, lparam) = (u64_at(16) as usize, u64_at(24) as isize);
            if hwnd == 0 {
                // PostThreadMessage: no window.
                crate::window::post_to(tid, crate::window::message(0, msg, wparam, lparam));
                continue;
            }
            let Some(w) = window(hwnd) else { continue };
            let m = crate::window::message(hwnd, msg, wparam, lparam);
            crate::window::post_to(w.thread, m);
        }
        if n < 32 {
            return;
        }
    }
}

/// PostThreadMessage to thread `tid` of any process (ours too: through
/// the kernel, which knows which threads have a message queue).
pub(crate) fn post_thread(tid: u32, msg: u32, wparam: usize, lparam: isize) -> i32 {
    const STATUS_INSUFFICIENT_RESOURCES: u32 = 0xc000_009a;
    const ERROR_INVALID_THREAD_ID: u32 = 1444;
    const ERROR_NOT_ENOUGH_QUOTA: u32 = 1816;
    // A User call: the calling thread has a message queue from now on
    // (so it can post to itself).
    note_queue();
    // SAFETY: a plain call.
    match unsafe { GalaWindowPostThread(tid, msg, wparam as u64, lparam as u64) } {
        0 => 1,
        STATUS_INSUFFICIENT_RESOURCES => {
            crate::window::set_last_error(ERROR_NOT_ENOUGH_QUOTA);
            0
        }
        STATUS_ACCESS_DENIED => {
            crate::window::set_last_error(ERROR_ACCESS_DENIED);
            0
        }
        _ => {
            crate::window::set_last_error(ERROR_INVALID_THREAD_ID);
            0
        }
    }
}

/// Tells the kernel the calling thread has a message queue (and whether a
/// message sent to it waits: true).
fn note_queue() -> bool {
    // SAFETY: no buffers: only whether one waits.
    unsafe {
        GalaWindowReceive(crate::window::me(), core::ptr::null_mut(), core::ptr::null_mut(), 0) == 0
    }
}

const WM_SETTEXT: u32 = 0x000c;
const WM_GETTEXT: u32 = 0x000d;
const WM_COPYDATA: u32 = 0x004a;
const STATUS_TIMEOUT: u32 = 0x102;
const INFINITE: u32 = 0xffff_ffff;
const ERROR_INVALID_WINDOW_HANDLE: u32 = 1400;
const ERROR_TIMEOUT: u32 = 1460;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
/// The most a sent message carries each way (the kernel's limit).
const MAX_DATA: usize = 64 * 1024;
/// COPYDATASTRUCT's size (dwData, cbData, lpData on x64).
const COPYDATA_SIZE: usize = 24;

/// How the kernel delivers a sent message (nt::window::SEND_*).
const SEND_NOTIFY: u32 = 1;
const SEND_CALLBACK: u32 = 2;
const SEND_ABORT_IF_HUNG: u32 = 4;

/// What InSendMessageEx reports.
const ISMEX_SEND: u32 = 1;
const ISMEX_NOTIFY: u32 = 2;
const ISMEX_CALLBACK: u32 = 4;
const ISMEX_REPLIED: u32 = 8;

/// SendMessageCallback's callback: (window, message, data, result).
pub(crate) type SendAsyncProc = extern "system" fn(Hwnd, u32, usize, isize);

/// How a message goes to another thread's window.
#[derive(Clone, Copy)]
pub(crate) enum Mode {
    /// SendMessage, SendMessageTimeout: wait up to `ms` (INFINITE) for
    /// the reply, handling messages sent to this thread meanwhile unless
    /// `block` (SMTO_BLOCK); `hung`: fail at once for a hung receiver
    /// (SMTO_ABORTIFHUNG).
    Wait { ms: u32, block: bool, hung: bool },
    /// SendNotifyMessage: nobody waits.
    Notify,
    /// SendMessageCallback: the callback gets the result later.
    Callback { callback: Option<SendAsyncProc>, data: usize },
}

/// SendMessage's way.
pub(crate) const WAIT: Mode = Mode::Wait { ms: INFINITE, block: false, hung: false };

/// SendMessage to another process's window: the message goes through
/// the kernel to the window's thread, which handles it the next time it
/// calls a message function; meanwhile messages sent to this thread are
/// handled (as Windows does, so two threads sending to each other don't
/// deadlock). Marshaled: WM_SETTEXT's string (in), WM_GETTEXT's buffer
/// (out), WM_COPYDATA's COPYDATASTRUCT and its buffer (in), the list
/// box's and combo box's strings (in: LB_ADDSTRING and the like; out:
/// LB_GETTEXT and CB_GETLBTEXT, sized by asking LB_GETTEXTLEN /
/// CB_GETLBTEXTLEN first), LB_GETITEMRECT's RECT and LB_GETSELITEMS's
/// array (out); other messages go as numbers. None when `hwnd` is no
/// other process's window.
pub(crate) fn send_remote(
    hwnd: Hwnd,
    msg: u32,
    wparam: usize,
    lparam: isize,
    mode: Mode,
) -> Option<Result<isize, u32>> {
    remote(hwnd)?;
    let (mut in_ptr, mut in_len, mut out_ptr, mut out_max) = (0u64, 0usize, 0u64, 0usize);
    match msg {
        LB_GETTEXT | CB_GETLBTEXT if lparam != 0 => {
            let len_msg = if msg == LB_GETTEXT { LB_GETTEXTLEN } else { CB_GETLBTEXTLEN };
            let n = match send_remote(hwnd, len_msg, wparam, 0, mode)? {
                Ok(n) => n,
                Err(e) => return Some(Err(e)),
            };
            if n < 0 {
                return Some(Ok(n));
            }
            (out_ptr, out_max) = (lparam as u64, (2 * (n as usize + 1)).min(MAX_DATA));
        }
        LB_GETITEMRECT if lparam != 0 => (out_ptr, out_max) = (lparam as u64, 16),
        crate::track::GALA_MENU if lparam != 0 => {
            (out_ptr, out_max) = (lparam as u64, crate::track::MENU_INFO_SIZE);
        }
        crate::tree::GALA_SETWINDOWPOS if lparam != 0 => {
            (in_ptr, in_len) = (lparam as u64, crate::tree::SETWINDOWPOS_SIZE);
        }
        LB_GETSELITEMS if lparam != 0 => {
            (out_ptr, out_max) = (lparam as u64, (4 * wparam).min(MAX_DATA));
        }
        WM_COPYDATA if lparam != 0 => return Some(send_copydata(hwnd, wparam, lparam, mode)),
        _ if in_string(msg) && lparam != 0 => {
            let text = lparam as *const u16;
            let mut n = 0;
            // SAFETY: the caller's NUL-terminated string.
            while n < MAX_DATA / 2 - 1 && unsafe { *text.add(n) } != 0 {
                n += 1;
            }
            (in_ptr, in_len) = (text as u64, 2 * (n + 1));
        }
        WM_GETTEXT if lparam != 0 => {
            (out_ptr, out_max) = (lparam as u64, (2 * wparam).min(MAX_DATA));
        }
        _ => {}
    }
    Some(send_through_kernel(hwnd, msg, wparam, lparam, (in_ptr, in_len), (out_ptr, out_max), mode))
}

/// WM_COPYDATA to another process's window: its COPYDATASTRUCT and the
/// buffer it points to go in one block (the receiver points lpData at its
/// copy of the buffer).
fn send_copydata(hwnd: Hwnd, wparam: usize, lparam: isize, mode: Mode) -> Result<isize, u32> {
    let cds = lparam as *const u8;
    // SAFETY: the caller's COPYDATASTRUCT.
    let (size, data) = unsafe {
        (cds.add(8).cast::<u32>().read_unaligned(), cds.add(16).cast::<usize>().read_unaligned())
    };
    let n = if data == 0 { 0 } else { (size as usize).min(MAX_DATA - COPYDATA_SIZE) };
    // SAFETY: a plain call.
    let block =
        unsafe { VirtualAlloc(core::ptr::null_mut(), COPYDATA_SIZE + n, 0x3000, 4) }.cast::<u8>();
    if block.is_null() {
        return Err(ERROR_NOT_ENOUGH_MEMORY);
    }
    // SAFETY: our block of COPYDATA_SIZE + n bytes; the caller's structure
    // and its buffer of (at least) n bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(cds, block, COPYDATA_SIZE);
        block.add(8).cast::<u32>().write_unaligned(n as u32);
        core::ptr::copy_nonoverlapping(data as *const u8, block.add(COPYDATA_SIZE), n);
    }
    let r = send_through_kernel(
        hwnd,
        WM_COPYDATA,
        wparam,
        lparam,
        (block as u64, COPYDATA_SIZE + n),
        (0, 0),
        mode,
    );
    // SAFETY: our allocation.
    unsafe { VirtualFree(block.cast(), 0, 0x8000) };
    r
}

/// SendMessage to a window of another thread of this process: as across
/// processes, the message goes to the window's thread and runs there the
/// next time it calls a message function, but nothing is marshaled (the
/// pointers are this process's). A window whose thread has ended is gone
/// with it (ERROR_INVALID_WINDOW_HANDLE).
pub(crate) fn send_to_thread(
    hwnd: Hwnd,
    msg: u32,
    wparam: usize,
    lparam: isize,
    mode: Mode,
) -> Result<isize, u32> {
    send_through_kernel(hwnd, msg, wparam, lparam, (0, 0), (0, 0), mode)
}

/// Sends through the kernel (GalaWindowSend) with the input `(pointer,
/// length)` copied in and up to `(pointer, max)` copied out; with
/// Mode::Wait waits for the reply, handling messages sent to this thread
/// meanwhile (unless it blocks); the others return at once.
fn send_through_kernel(
    hwnd: Hwnd,
    msg: u32,
    wparam: usize,
    lparam: isize,
    (in_ptr, in_len): (u64, usize),
    (out_ptr, out_max): (u64, usize),
    mode: Mode,
) -> Result<isize, u32> {
    let flags = match mode {
        Mode::Wait { hung: true, .. } => SEND_ABORT_IF_HUNG,
        Mode::Wait { .. } => 0,
        Mode::Notify => SEND_NOTIFY,
        Mode::Callback { .. } => SEND_CALLBACK,
    };
    // A callback needs a place to wait in before the message goes.
    let slot = match mode {
        Mode::Callback { .. } => Some(claim_callback().ok_or(ERROR_NOT_ENOUGH_MEMORY)?),
        _ => None,
    };
    let mut req = [0u8; 48];
    req[0..8].copy_from_slice(&(hwnd as u64).to_le_bytes());
    req[8..12].copy_from_slice(&msg.to_le_bytes());
    req[12..16].copy_from_slice(&(out_max as u32).to_le_bytes());
    req[16..24].copy_from_slice(&(wparam as u64).to_le_bytes());
    req[24..32].copy_from_slice(&(lparam as u64).to_le_bytes());
    req[32..40].copy_from_slice(&in_ptr.to_le_bytes());
    req[40..44].copy_from_slice(&(in_len as u32).to_le_bytes());
    req[44..48].copy_from_slice(&flags.to_le_bytes());
    let mut id = 0u64;
    // SAFETY: our request; the input is the caller's (or our copy).
    let status = unsafe { GalaWindowSend(req.as_ptr(), &mut id) };
    if status != 0 {
        if let Some(c) = slot {
            free_callback(c);
        }
        return Err(match status {
            STATUS_TIMEOUT => ERROR_TIMEOUT,
            STATUS_ACCESS_DENIED => ERROR_ACCESS_DENIED,
            _ => ERROR_INVALID_WINDOW_HANDLE,
        });
    }
    let (ms, block) = match mode {
        Mode::Wait { ms, block, .. } => (ms, block),
        Mode::Notify => return Ok(1),
        Mode::Callback { callback, data } => {
            if let Some(c) = slot {
                c.hwnd.store(hwnd, Ordering::Relaxed);
                c.msg.store(msg, Ordering::Relaxed);
                c.callback.store(callback.map_or(0, |p| p as usize), Ordering::Relaxed);
                c.data.store(data, Ordering::Relaxed);
                c.id.store(id, Ordering::Release);
            }
            return Ok(1);
        }
    };
    // SAFETY: a plain call.
    let start = unsafe { GetTickCount() };
    loop {
        // SAFETY: a plain call.
        let gone = unsafe { GetTickCount() }.wrapping_sub(start);
        let left = if ms == INFINITE { INFINITE } else { ms.saturating_sub(gone) };
        let (mut result, mut len) = (0i64, 0u32);
        // SAFETY: our outputs; the output goes into the caller's buffer
        // of `out_max` bytes (WM_GETTEXT's).
        let status = unsafe {
            GalaWindowSendWait(id, left.min(10), &mut result, out_ptr as *mut u8, &mut len)
        };
        match status {
            0 => return Ok(result as isize),
            STATUS_TIMEOUT if left == 0 => {
                // SAFETY: a plain call: the reply, when it comes, goes.
                unsafe { GalaWindowSendAbandon(id) };
                return Err(ERROR_TIMEOUT);
            }
            STATUS_TIMEOUT if block => {}
            STATUS_TIMEOUT => take_sent(),
            _ => return Err(ERROR_INVALID_WINDOW_HANDLE),
        }
    }
}

/// What a thread is handling of a message another thread sent it (for
/// ReplyMessage and InSendMessageEx): the message's id, how it was sent
/// (ISMEX_*), and its output buffer. Each slot belongs to one thread
/// (claimed once, freed when it ends); only that thread changes the rest.
struct Receiving {
    thread: AtomicU32,
    id: AtomicU64,
    ismex: AtomicU32,
    out: AtomicUsize,
    out_max: AtomicU32,
}

const THREAD_SLOTS: usize = 64;
static RECEIVING: [Receiving; THREAD_SLOTS] = [const {
    Receiving {
        thread: AtomicU32::new(0),
        id: AtomicU64::new(0),
        ismex: AtomicU32::new(0),
        out: AtomicUsize::new(0),
        out_max: AtomicU32::new(0),
    }
}; THREAD_SLOTS];

/// The calling thread's Receiving slot, claimed if `claim`.
fn receiving(claim: bool) -> Option<&'static Receiving> {
    let me = crate::window::me();
    if let Some(r) = RECEIVING.iter().find(|r| r.thread.load(Ordering::Relaxed) == me) {
        return Some(r);
    }
    if !claim {
        return None;
    }
    RECEIVING
        .iter()
        .find(|r| r.thread.compare_exchange(0, me, Ordering::AcqRel, Ordering::Relaxed).is_ok())
}

/// Handles the messages other threads (ours or other processes') sent to
/// this thread's windows: each goes to its window's procedure with its
/// marshaled buffers, and the result (and output) goes back, unless
/// ReplyMessage answered already.
pub(crate) fn take_sent() {
    let me = crate::window::me();
    loop {
        // Most of the time nothing waits: ask before allocating.
        if !note_queue() {
            return;
        }
        // SAFETY: plain calls; the buffers are ours.
        let buf =
            unsafe { VirtualAlloc(core::ptr::null_mut(), 2 * MAX_DATA, 0x3000, 4) }.cast::<u8>();
        if buf.is_null() {
            return;
        }
        let mut head = [0u8; 48];
        // SAFETY: our header and input buffer of MAX_DATA bytes.
        let status = unsafe { GalaWindowReceive(me, head.as_mut_ptr(), buf, MAX_DATA as u32) };
        if status != 0 {
            // SAFETY: our allocation.
            unsafe { VirtualFree(buf.cast(), 0, 0x8000) };
            return;
        }
        let u64_at = |at: usize| u64::from_le_bytes(head[at..at + 8].try_into().unwrap_or([0; 8]));
        let u32_at = |at: usize| u32::from_le_bytes(head[at..at + 4].try_into().unwrap_or([0; 4]));
        let (id, hwnd, msg) = (u64_at(0), u64_at(8) as Hwnd, u32_at(16));
        let (out_max, wparam, lparam, in_len) =
            (u32_at(20) as usize, u64_at(24) as usize, u64_at(32) as isize, u32_at(40) as usize);
        let flags = u32_at(44);
        // SAFETY: the second half of our allocation.
        let out = unsafe { buf.add(MAX_DATA) };
        if msg == WM_COPYDATA && in_len >= COPYDATA_SIZE {
            // Our copy of the buffer follows our copy of the structure.
            // SAFETY: our input buffer, at least COPYDATA_SIZE bytes.
            unsafe {
                let size = buf.add(8).cast::<u32>().read_unaligned();
                let data = if size == 0 { 0 } else { buf.add(COPYDATA_SIZE) as usize };
                buf.add(16).cast::<usize>().write_unaligned(data);
            }
        }
        let lparam = if in_len > 0 {
            buf as isize
        } else if out_max > 0 {
            out as isize
        } else {
            lparam
        };
        let ismex = if flags & SEND_NOTIFY != 0 {
            ISMEX_NOTIFY
        } else if flags & SEND_CALLBACK != 0 {
            ISMEX_CALLBACK
        } else {
            ISMEX_SEND
        };
        // What this thread was handling before (a message sent while it
        // waited for its own send's reply) comes back afterwards.
        let slot = receiving(true);
        let before = slot.map(|r| {
            let before = (
                r.id.load(Ordering::Relaxed),
                r.ismex.load(Ordering::Relaxed),
                r.out.load(Ordering::Relaxed),
                r.out_max.load(Ordering::Relaxed),
            );
            r.id.store(id, Ordering::Relaxed);
            r.ismex.store(ismex, Ordering::Relaxed);
            r.out.store(out as usize, Ordering::Relaxed);
            r.out_max.store(out_max as u32, Ordering::Relaxed);
            before
        });
        let result = crate::window::SendMessageW(hwnd, msg, wparam, lparam);
        let replied = slot.is_some_and(|r| r.ismex.load(Ordering::Relaxed) & ISMEX_REPLIED != 0);
        if let (Some(r), Some((id, ismex, out, out_max))) = (slot, before) {
            r.id.store(id, Ordering::Relaxed);
            r.ismex.store(ismex, Ordering::Relaxed);
            r.out.store(out, Ordering::Relaxed);
            r.out_max.store(out_max, Ordering::Relaxed);
        }
        // SAFETY: our output buffer of `out_max` bytes; then our allocation.
        unsafe {
            if !replied {
                GalaWindowReply(id, result as i64, out, out_max as u32);
            }
            VirtualFree(buf.cast(), 0, 0x8000);
        }
    }
}

/// ReplyMessage(result): the sender of the message this thread is
/// handling gets `result` now (and the output as it stands); the
/// procedure's own return goes nowhere. FALSE when this thread is not
/// handling a message another thread sent.
#[unsafe(no_mangle)]
pub extern "system" fn ReplyMessage(result: isize) -> i32 {
    let Some(r) = receiving(false) else { return 0 };
    let id = r.id.load(Ordering::Relaxed);
    if id == 0 {
        return 0;
    }
    let ismex = r.ismex.load(Ordering::Relaxed);
    if ismex & ISMEX_REPLIED == 0 {
        r.ismex.store(ismex | ISMEX_REPLIED, Ordering::Relaxed);
        let (out, out_max) = (r.out.load(Ordering::Relaxed), r.out_max.load(Ordering::Relaxed));
        // SAFETY: the message's output buffer (ours, `out_max` bytes).
        unsafe { GalaWindowReply(id, result as i64, out as *const u8, out_max) };
    }
    1
}

/// InSendMessageEx(reserved): how the message this thread is handling
/// was sent by another thread: ISMEX_SEND (SendMessage,
/// SendMessageTimeout), ISMEX_NOTIFY, ISMEX_CALLBACK, with ISMEX_REPLIED
/// once ReplyMessage answered; ISMEX_NOSEND (0) for none.
#[unsafe(no_mangle)]
pub extern "system" fn InSendMessageEx(_reserved: usize) -> u32 {
    receiving(false)
        .filter(|r| r.id.load(Ordering::Relaxed) != 0)
        .map_or(0, |r| r.ismex.load(Ordering::Relaxed))
}

/// InSendMessage: whether this thread is handling a message another
/// thread sent with SendMessage (and not replied to yet).
#[unsafe(no_mangle)]
pub extern "system" fn InSendMessage() -> i32 {
    i32::from(InSendMessageEx(0) & (ISMEX_SEND | ISMEX_REPLIED) == ISMEX_SEND)
}

/// A SendMessageCallback waiting for its reply: the thread that sent it
/// (0: free), the message's id (0 until sent), and what the callback gets.
struct Callback {
    thread: AtomicU32,
    id: AtomicU64,
    hwnd: AtomicUsize,
    msg: AtomicU32,
    callback: AtomicUsize,
    data: AtomicUsize,
}

const CALLBACKS_MAX: usize = 64;
static CALLBACKS: [Callback; CALLBACKS_MAX] = [const {
    Callback {
        thread: AtomicU32::new(0),
        id: AtomicU64::new(0),
        hwnd: AtomicUsize::new(0),
        msg: AtomicU32::new(0),
        callback: AtomicUsize::new(0),
        data: AtomicUsize::new(0),
    }
}; CALLBACKS_MAX];
/// How many CALLBACKS slots are taken (the pump looks only when some are).
static CALLBACKS_TAKEN: AtomicUsize = AtomicUsize::new(0);

fn claim_callback() -> Option<&'static Callback> {
    let me = crate::window::me();
    let c = CALLBACKS
        .iter()
        .find(|c| c.thread.compare_exchange(0, me, Ordering::AcqRel, Ordering::Relaxed).is_ok())?;
    c.id.store(0, Ordering::Relaxed);
    CALLBACKS_TAKEN.fetch_add(1, Ordering::AcqRel);
    Some(c)
}

fn free_callback(c: &Callback) {
    c.id.store(0, Ordering::Relaxed);
    c.thread.store(0, Ordering::Release);
    CALLBACKS_TAKEN.fetch_sub(1, Ordering::AcqRel);
}

/// Calls the callbacks of this thread's SendMessageCallback messages that
/// have their reply (GetMessage, PeekMessage and WaitMessage call this);
/// one whose receiver ended without replying is dropped.
pub(crate) fn take_callbacks() {
    if CALLBACKS_TAKEN.load(Ordering::Acquire) == 0 {
        return;
    }
    let me = crate::window::me();
    for c in CALLBACKS.iter() {
        if c.thread.load(Ordering::Acquire) != me {
            continue;
        }
        let id = c.id.load(Ordering::Acquire);
        if id == 0 {
            continue;
        }
        let (mut result, mut len) = (0i64, 0u32);
        // SAFETY: our outputs; no output buffer (these messages carry no
        // pointers).
        let status =
            unsafe { GalaWindowSendWait(id, 0, &mut result, core::ptr::null_mut(), &mut len) };
        if status == STATUS_TIMEOUT {
            continue;
        }
        let (hwnd, msg) = (c.hwnd.load(Ordering::Relaxed), c.msg.load(Ordering::Relaxed));
        let (callback, data) = (c.callback.load(Ordering::Relaxed), c.data.load(Ordering::Relaxed));
        free_callback(c);
        if status == 0 && callback != 0 {
            // SAFETY: the SENDASYNCPROC SendMessageCallback was given.
            let p = unsafe { core::mem::transmute::<usize, SendAsyncProc>(callback) };
            p(hwnd, msg, data, result as isize);
        }
    }
}

/// The calling thread is ending: its slots go (its callbacks are not
/// called; the kernel drops its sends).
pub(crate) fn thread_ended() {
    let me = crate::window::me();
    for c in CALLBACKS.iter() {
        if c.thread.load(Ordering::Acquire) == me {
            free_callback(c);
        }
    }
    if let Some(r) = receiving(false) {
        r.id.store(0, Ordering::Relaxed);
        r.ismex.store(0, Ordering::Relaxed);
        r.thread.store(0, Ordering::Release);
    }
}

// ---- UIPI's message filters -------------------------------------------

/// CHANGEFILTERSTRUCT: cbSize, ExtStatus.
#[repr(C)]
pub struct ChangeFilterStruct {
    size: u32,
    ext_status: u32,
}

/// The kernel's filter call's status as the functions' result.
fn filter_result(status: u32) -> i32 {
    const STATUS_INVALID_HANDLE: u32 = 0xc000_0008;
    const ERROR_INVALID_PARAMETER: u32 = 87;
    match status {
        0 => 1,
        STATUS_ACCESS_DENIED => {
            crate::window::set_last_error(ERROR_ACCESS_DENIED);
            0
        }
        STATUS_INVALID_HANDLE => {
            crate::window::set_last_error(ERROR_INVALID_WINDOW_HANDLE);
            0
        }
        _ => {
            crate::window::set_last_error(ERROR_INVALID_PARAMETER);
            0
        }
    }
}

/// ChangeWindowMessageFilterEx: lets `message` reach `hwnd` (one of ours)
/// from processes of lower integrity (MSGFLT_ALLOW 1), blocks it again
/// (MSGFLT_DISALLOW 2) or resets the window's filter (MSGFLT_RESET 0);
/// CHANGEFILTERSTRUCT's ExtStatus says what was already so (its cbSize
/// must be right: ERROR_INVALID_PARAMETER). Another process's window or a
/// caller at Low integrity: ERROR_ACCESS_DENIED.
#[unsafe(no_mangle)]
pub extern "system" fn ChangeWindowMessageFilterEx(
    hwnd: Hwnd,
    message: u32,
    action: u32,
    change: *mut ChangeFilterStruct,
) -> i32 {
    const ERROR_INVALID_PARAMETER: u32 = 87;
    // SAFETY: the caller's structure, if any.
    if !change.is_null() && unsafe { (*change).size } as usize != size_of::<ChangeFilterStruct>() {
        crate::window::set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    if hwnd == 0 {
        crate::window::set_last_error(ERROR_INVALID_WINDOW_HANDLE);
        return 0;
    }
    let mut ext = 0u32;
    // SAFETY: our output.
    let status = unsafe { GalaWindowFilter(hwnd as u64, message, action, &mut ext) };
    let ok = filter_result(status);
    if ok != 0 && !change.is_null() {
        // SAFETY: the caller's structure (its size checked above).
        unsafe { (*change).ext_status = ext };
    }
    ok
}

/// ChangeWindowMessageFilter: the process-wide filter (MSGFLT_ADD 1 lets
/// `message` through from lower integrity levels, MSGFLT_REMOVE 2 blocks
/// it again). Fails at Low integrity.
#[unsafe(no_mangle)]
pub extern "system" fn ChangeWindowMessageFilter(message: u32, flag: u32) -> i32 {
    const ERROR_INVALID_PARAMETER: u32 = 87;
    if flag != 1 && flag != 2 {
        crate::window::set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    // SAFETY: no output.
    filter_result(unsafe { GalaWindowFilter(0, message, flag, core::ptr::null_mut()) })
}
