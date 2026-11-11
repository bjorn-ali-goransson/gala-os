//! The notification area: Shell_NotifyIconW/A and
//! Shell_NotifyIconGetRect, as Microsoft documents them, over the
//! kernel's tray (GalaTrayIcon; ARCHITECTURE "The notification area").
//! NOTIFYICONDATA is read as far as its cbSize goes: the four documented
//! sizes (V1 with a 64-character tip, V2 adding the 128-character tip,
//! state and balloon, V3 the GUID, the full one the balloon's icon); any
//! other size counts as V1, as Wine's systray tests record of Windows
//! (an invalid cbSize works, with the first version's fields). The icon
//! goes to the kernel as pixels (user32's GalaIconArgb), which the
//! taskbar draws at 16 x 16. Where gala runs no shell (a bare desktop:
//! no screen, the server profile) there is no notification area and the
//! calls fail as on Windows without Explorer (recorded).

use crate::unsupported;
use core::ffi::c_void;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaTrayIcon(op: u32, request: *const u8, length: usize, rect: *mut i32) -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn MultiByteToWideChar(cp: u32, flags: u32, s: *const u8, n: i32, w: *mut u16, m: i32) -> i32;
    fn LoadLibraryW(name: *const u16) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> *const c_void;
}

const NIM_SETVERSION: u32 = 4;
const NIF_TIP: u32 = 0x04;
const NIF_INFO: u32 = 0x10;
const NIF_GUID: u32 = 0x20;
const NIF_REALTIME: u32 = 0x40;
const NIIF_USER: u32 = 0x04;
const NIIF_LARGE_ICON: u32 = 0x20;
/// What the V1 structure may set (NIF_MESSAGE, NIF_ICON, NIF_TIP).
const V1_FLAGS: u32 = 0x07;

const STATUS_NOT_SUPPORTED: u32 = 0xc000_00bb;
const S_OK: i32 = 0;
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
const E_FAIL: i32 = 0x8000_4005u32 as i32;

/// NOTIFYICONDATAW (shellapi.h); pointer-sized fields as `usize`, so the
/// layout is right for both widths.
#[repr(C)]
#[derive(Clone, Copy)]
struct DataW {
    cb_size: u32,
    hwnd: usize,
    uid: u32,
    flags: u32,
    callback: u32,
    icon: usize,
    tip: [u16; 128],
    state: u32,
    state_mask: u32,
    info: [u16; 256],
    /// uTimeout or uVersion.
    version: u32,
    info_title: [u16; 64],
    info_flags: u32,
    guid: [u32; 4],
    balloon_icon: usize,
}

/// NOTIFYICONDATAA.
#[repr(C)]
#[derive(Clone, Copy)]
struct DataA {
    cb_size: u32,
    hwnd: usize,
    uid: u32,
    flags: u32,
    callback: u32,
    icon: usize,
    tip: [u8; 128],
    state: u32,
    state_mask: u32,
    info: [u8; 256],
    version: u32,
    info_title: [u8; 64],
    info_flags: u32,
    guid: [u32; 4],
    balloon_icon: usize,
}

/// NOTIFYICONIDENTIFIER.
#[repr(C)]
pub struct Identifier {
    cb_size: u32,
    hwnd: usize,
    uid: u32,
    guid: [u32; 4],
}

/// NOTIFYICONDATAW_V1_SIZE, _V2_SIZE, _V3_SIZE and sizeof: x64 168, 952,
/// 968, 976; x86 152, 936, 952, 956 (checked below).
const W_SIZES: [usize; 4] = [
    core::mem::offset_of!(DataW, tip) + 2 * 64,
    core::mem::offset_of!(DataW, guid),
    core::mem::offset_of!(DataW, balloon_icon),
    size_of::<DataW>(),
];
/// NOTIFYICONDATAA's: x64 104, 504, 520, 528; x86 88, 488, 504, 508.
const A_SIZES: [usize; 4] = [
    core::mem::offset_of!(DataA, tip) + 64,
    core::mem::offset_of!(DataA, guid),
    core::mem::offset_of!(DataA, balloon_icon),
    size_of::<DataA>(),
];

#[cfg(target_arch = "x86_64")]
const _: () = assert!(
    W_SIZES[0] == 168
        && W_SIZES[1] == 952
        && W_SIZES[2] == 968
        && W_SIZES[3] == 976
        && A_SIZES[0] == 104
        && A_SIZES[1] == 504
        && A_SIZES[2] == 520
        && A_SIZES[3] == 528
        && size_of::<Identifier>() == 40
);
#[cfg(target_arch = "x86")]
const _: () = assert!(
    W_SIZES[0] == 152
        && W_SIZES[1] == 936
        && W_SIZES[2] == 952
        && W_SIZES[3] == 956
        && A_SIZES[0] == 88
        && A_SIZES[1] == 488
        && A_SIZES[2] == 504
        && A_SIZES[3] == 508
        && size_of::<Identifier>() == 28
);

/// The bytes of the caller's structure that count: cbSize if it is one
/// of the documented sizes, else the first version's.
fn counted(cb: u32, sizes: [usize; 4]) -> usize {
    let cb = cb as usize;
    if sizes.contains(&cb) { cb } else { sizes[0] }
}

/// GalaTrayIcon's request (kernel/src/nt/syscalls.rs `tray_request`).
const TEXT_AT: usize = 64;
const TIP_LEN: usize = 128;
const TITLE_LEN: usize = 64;
const INFO_LEN: usize = 256;
const ICON_AT: usize = TEXT_AT + 2 * (TIP_LEN + TITLE_LEN + INFO_LEN);
const MAX_SIDE: usize = 64;
const REQUEST: usize = ICON_AT + 4 * MAX_SIDE * MAX_SIDE;

struct Request([u8; REQUEST]);

impl Request {
    fn put32(&mut self, at: usize, v: u32) {
        self.0[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn text(&mut self, at: usize, room: usize, text: &[u16]) {
        let n = text.iter().position(|&c| c == 0).unwrap_or(text.len()).min(room - 1);
        for (i, &c) in text[..n].iter().enumerate() {
            self.0[at + 2 * i..at + 2 * i + 2].copy_from_slice(&c.to_le_bytes());
        }
    }
}

/// user32's GalaIconArgb: icon `h` as straight-alpha pixels (at most
/// 64 x 64) and their side.
fn icon_pixels(h: usize, out: &mut [u32; MAX_SIDE * MAX_SIDE]) -> Option<u32> {
    const USER32: [u16; 11] = [117, 115, 101, 114, 51, 50, 46, 100, 108, 108, 0];
    // SAFETY: NUL-terminated names; the function's documented signature
    // (user/user32/src/icon.rs) with our buffer of 64 x 64.
    unsafe {
        let m = LoadLibraryW(USER32.as_ptr());
        if m == 0 {
            return None;
        }
        let f = GetProcAddress(m, c"GalaIconArgb".as_ptr().cast());
        if f.is_null() {
            return None;
        }
        let f: extern "system" fn(usize, *mut u32, *mut u32) -> i32 = core::mem::transmute(f);
        let mut side = 0u32;
        (f(h, out.as_mut_ptr(), &mut side) != 0 && side as usize <= MAX_SIDE).then_some(side)
    }
}

/// Shell_NotifyIcon on a NOTIFYICONDATAW whose counted part is `d`.
fn notify(message: u32, d: &DataW, size: usize, sizes: [usize; 4]) -> i32 {
    if message > NIM_SETVERSION {
        unsupported(b"shell32.dll!Shell_NotifyIcon(message)", message.into());
        return 0;
    }
    let v1 = size == sizes[0];
    // The first version has no state, balloon, GUID or SHOWTIP.
    let mut flags = if v1 { d.flags & V1_FLAGS } else { d.flags };
    if size < sizes[2] {
        flags &= !NIF_GUID;
    }
    if flags & NIF_REALTIME != 0 {
        flags &= !NIF_REALTIME; // shown at once anyway
    }
    let mut r = Request([0; REQUEST]);
    r.put32(0, 1);
    r.put32(4, flags);
    r.0[8..16].copy_from_slice(&(d.hwnd as u64).to_le_bytes());
    r.put32(16, d.uid);
    r.put32(20, d.callback);
    if !v1 {
        r.put32(24, d.state);
        r.put32(28, d.state_mask);
        r.put32(32, d.version);
        r.put32(36, d.info_flags);
        r.text(TEXT_AT + 2 * TIP_LEN, TITLE_LEN, &d.info_title);
        r.text(TEXT_AT + 2 * (TIP_LEN + TITLE_LEN), INFO_LEN, &d.info);
        if flags & NIF_INFO != 0 && d.info_flags & (NIIF_USER | NIIF_LARGE_ICON) != 0 {
            unsupported(b"shell32.dll!Shell_NotifyIcon(balloon icon)", d.info_flags.into());
        }
    }
    if flags & NIF_GUID != 0 {
        for (i, w) in d.guid.iter().enumerate() {
            r.put32(40 + 4 * i, *w);
        }
    }
    if flags & NIF_TIP != 0 {
        r.text(TEXT_AT, if v1 { 64 } else { TIP_LEN }, &d.tip);
    }
    if flags & 0x02 != 0 && d.icon != 0 {
        let mut px = [0u32; MAX_SIDE * MAX_SIDE];
        match icon_pixels(d.icon, &mut px) {
            Some(side) => {
                r.put32(56, side);
                for (i, p) in px[..(side * side) as usize].iter().enumerate() {
                    r.0[ICON_AT + 4 * i..ICON_AT + 4 * i + 4].copy_from_slice(&p.to_le_bytes());
                }
            }
            None => unsupported(b"shell32.dll!Shell_NotifyIcon(icon not readable)", d.icon as u64),
        }
    }
    // SAFETY: our request of REQUEST bytes.
    let status = unsafe { GalaTrayIcon(message, r.0.as_ptr(), REQUEST, core::ptr::null_mut()) };
    if status == STATUS_NOT_SUPPORTED {
        unsupported(b"shell32.dll!Shell_NotifyIcon(no notification area)", message.into());
    }
    i32::from(status == 0)
}

/// Shell_NotifyIconW(dwMessage, lpData): NIM_ADD, NIM_MODIFY,
/// NIM_DELETE, NIM_SETFOCUS, NIM_SETVERSION. TRUE when done; FALSE for
/// an icon added twice, one that is not there, or no notification area.
#[unsafe(no_mangle)]
pub extern "system" fn Shell_NotifyIconW(message: u32, data: *const DataWOpaque) -> i32 {
    if data.is_null() {
        return 0;
    }
    let data = data.cast::<u8>();
    // SAFETY: the caller's structure, cbSize first; only the bytes its
    // counted size covers are read.
    let (size, d) = unsafe {
        let size = counted(data.cast::<u32>().read_unaligned(), W_SIZES);
        let mut d: DataW = core::mem::zeroed();
        core::ptr::copy_nonoverlapping(data, (&raw mut d).cast::<u8>(), size);
        (size, d)
    };
    notify(message, &d, size, W_SIZES)
}

/// An opaque NOTIFYICONDATA (the export's parameter type).
pub enum DataWOpaque {}

/// Shell_NotifyIconA: the same with ANSI texts (the ANSI code page).
#[unsafe(no_mangle)]
pub extern "system" fn Shell_NotifyIconA(message: u32, data: *const DataWOpaque) -> i32 {
    if data.is_null() {
        return 0;
    }
    let data = data.cast::<u8>();
    // SAFETY: as Shell_NotifyIconW, with the ANSI structure.
    let (size, a) = unsafe {
        let size = counted(data.cast::<u32>().read_unaligned(), A_SIZES);
        let mut a: DataA = core::mem::zeroed();
        core::ptr::copy_nonoverlapping(data, (&raw mut a).cast::<u8>(), size);
        (size, a)
    };
    // SAFETY: zero is a valid DataW.
    let mut w: DataW = unsafe { core::mem::zeroed() };
    w.hwnd = a.hwnd;
    w.uid = a.uid;
    w.flags = a.flags;
    w.callback = a.callback;
    w.icon = a.icon;
    w.state = a.state;
    w.state_mask = a.state_mask;
    w.version = a.version;
    w.info_flags = a.info_flags;
    w.guid = a.guid;
    w.balloon_icon = a.balloon_icon;
    let widen = |s: &[u8], out: &mut [u16]| {
        let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
        if n > 0 {
            // SAFETY: our buffers with their lengths.
            unsafe {
                MultiByteToWideChar(0, 0, s.as_ptr(), n as i32, out.as_mut_ptr(), out.len() as i32)
            };
        }
    };
    let v1 = size == A_SIZES[0];
    widen(&a.tip[..if v1 { 64 } else { 128 }], &mut w.tip[..127]);
    if !v1 {
        widen(&a.info, &mut w.info[..255]);
        widen(&a.info_title, &mut w.info_title[..63]);
    }
    // The same version of the wide structure.
    let wsize = W_SIZES[A_SIZES.iter().position(|&s| s == size).unwrap_or(0)];
    notify(message, &w, wsize, W_SIZES)
}

/// Shell_NotifyIconGetRect(identifier, iconLocation): the icon's screen
/// rectangle (S_OK); E_FAIL when there is no such icon shown (or no
/// notification area); E_INVALIDARG for NULL pointers. The icon is named
/// by its GUID when guidItem is not GUID_NULL, else by hWnd and uID.
#[unsafe(no_mangle)]
pub extern "system" fn Shell_NotifyIconGetRect(id: *const Identifier, rect: *mut i32) -> i32 {
    if id.is_null() || rect.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's NOTIFYICONIDENTIFIER.
    let id = unsafe { &*id };
    let mut r = Request([0; REQUEST]);
    r.put32(0, 1);
    r.0[8..16].copy_from_slice(&(id.hwnd as u64).to_le_bytes());
    r.put32(16, id.uid);
    if id.guid != [0; 4] {
        r.put32(4, NIF_GUID);
        for (i, w) in id.guid.iter().enumerate() {
            r.put32(40 + 4 * i, *w);
        }
    }
    let mut xywh = [0i32; 4];
    // SAFETY: our request and rectangle.
    let status = unsafe { GalaTrayIcon(0x10, r.0.as_ptr(), REQUEST, xywh.as_mut_ptr()) };
    if status != 0 {
        if status == STATUS_NOT_SUPPORTED {
            unsupported(b"shell32.dll!Shell_NotifyIconGetRect(no notification area)", 0);
        }
        return E_FAIL;
    }
    let [x, y, w, h] = xywh;
    // SAFETY: the caller's RECT (left, top, right, bottom).
    unsafe {
        *rect = x;
        *rect.add(1) = y;
        *rect.add(2) = x + w;
        *rect.add(3) = y + h;
    }
    S_OK
}
