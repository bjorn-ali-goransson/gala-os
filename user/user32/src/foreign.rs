//! Windows across processes: the kernel's window registry (nt::window),
//! which holds every window of the desktop. user32 registers each window
//! it creates there (the HWND comes from the kernel, so HWNDs are unique
//! across processes), mirrors the fields other processes may ask about
//! (`sync`), and answers questions about other processes' windows from
//! it (`query`, `children`). See docs/ARCHITECTURE.md, "Plan: windows
//! across processes".

use crate::window::{Hwnd, MAX_WINDOWS, window, with};
use core::sync::atomic::{AtomicUsize, Ordering};

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaWindowRegister(info: *const u8, hwnd: *mut u64) -> u32;
    fn GalaWindowUpdate(info: *const u8) -> u32;
    fn GalaWindowUnregister(hwnd: u64) -> u32;
    fn GalaWindowQuery(hwnd: u64, info: *mut u8) -> u32;
    fn GalaWindowChildren(parent: u64, hwnds: *mut u64, max: u32, count: *mut u32) -> u32;
}

const CLASS_CHARS: usize = 64;
const TEXT_CHARS: usize = 256;
/// The record's size (nt::window::INFO_SIZE).
const INFO_SIZE: usize = 88 + 2 * CLASS_CHARS + 2 * TEXT_CHARS;
const FLAG_VISIBLE: u32 = 1;
const FLAG_ENABLED: u32 = 2;

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
    pub id: usize,
    pub pid: u32,
    pub tid: u32,
    pub style: u32,
    pub ex_style: u32,
    pub rect: [i32; 4],
    pub visible: bool,
    pub enabled: bool,
    pub z: i64,
    pub class: [u16; CLASS_CHARS],
    pub class_len: usize,
    pub text: [u16; TEXT_CHARS],
    pub text_len: usize,
}

impl Info {
    fn bytes(&self) -> [u8; INFO_SIZE] {
        let mut b = [0u8; INFO_SIZE];
        b[0..8].copy_from_slice(&(self.hwnd as u64).to_le_bytes());
        b[8..16].copy_from_slice(&(self.parent as u64).to_le_bytes());
        b[24..32].copy_from_slice(&(self.id as u64).to_le_bytes());
        b[36..40].copy_from_slice(&self.tid.to_le_bytes());
        b[40..44].copy_from_slice(&self.style.to_le_bytes());
        b[44..48].copy_from_slice(&self.ex_style.to_le_bytes());
        for (k, v) in self.rect.iter().enumerate() {
            b[48 + 4 * k..52 + 4 * k].copy_from_slice(&v.to_le_bytes());
        }
        let flags = if self.visible { FLAG_VISIBLE } else { 0 }
            | if self.enabled { FLAG_ENABLED } else { 0 };
        b[64..68].copy_from_slice(&flags.to_le_bytes());
        b[68..72].copy_from_slice(&(self.class_len as u32).to_le_bytes());
        b[72..80].copy_from_slice(&self.z.to_le_bytes());
        b[80..84].copy_from_slice(&(self.text_len as u32).to_le_bytes());
        for (k, c) in self.class.iter().enumerate() {
            b[88 + 2 * k..90 + 2 * k].copy_from_slice(&c.to_le_bytes());
        }
        let t = 88 + 2 * CLASS_CHARS;
        for (k, c) in self.text.iter().enumerate() {
            b[t + 2 * k..t + 2 + 2 * k].copy_from_slice(&c.to_le_bytes());
        }
        b
    }

    fn from_bytes(b: &[u8; INFO_SIZE]) -> Info {
        let u64_at = |at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap_or([0; 8]));
        let u32_at = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap_or([0; 4]));
        let mut info = Info {
            hwnd: u64_at(0) as Hwnd,
            parent: u64_at(8) as Hwnd,
            id: u64_at(24) as usize,
            pid: u32_at(32),
            tid: u32_at(36),
            style: u32_at(40),
            ex_style: u32_at(44),
            rect: [u32_at(48) as i32, u32_at(52) as i32, u32_at(56) as i32, u32_at(60) as i32],
            visible: u32_at(64) & FLAG_VISIBLE != 0,
            enabled: u32_at(64) & FLAG_ENABLED != 0,
            class_len: (u32_at(68) as usize).min(CLASS_CHARS),
            z: u64_at(72) as i64,
            text_len: (u32_at(80) as usize).min(TEXT_CHARS),
            class: [0; CLASS_CHARS],
            text: [0; TEXT_CHARS],
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
        id: w.id,
        pid: 0,
        tid: w.thread,
        style: w.style,
        ex_style: w.ex_style,
        rect: [0; 4],
        visible: w.visible,
        enabled: !w.disabled,
        z: w.z,
        class: [0; CLASS_CHARS],
        class_len: 0,
        text: [0; TEXT_CHARS],
        text_len: 0,
    };
    info.class_len = spelled.iter().position(|&c| c == 0).unwrap_or(spelled.len());
    info.class[..info.class_len].copy_from_slice(&spelled[..info.class_len]);
    info.text_len = w.title.iter().position(|&c| c == 0).unwrap_or(w.title.len());
    info.text[..info.text_len].copy_from_slice(&w.title[..info.text_len]);
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
        id: 0,
        pid: 0,
        tid: 0,
        style: 0,
        ex_style: 0,
        rect: [0; 4],
        visible: false,
        enabled: true,
        z: 0,
        class: [0; CLASS_CHARS],
        class_len: 0,
        text: [0; TEXT_CHARS],
        text_len: 0,
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
