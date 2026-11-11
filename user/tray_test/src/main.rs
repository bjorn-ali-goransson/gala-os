//! The notification area and the icons it shows (ROADMAP OF: Office's
//! Click-to-Run client places its progress flyout by its tray icon).
//! Expected results are Windows' as Microsoft documents Shell_NotifyIcon,
//! Shell_NotifyIconGetRect, LoadIconMetric and CreateIconFromResourceEx,
//! and as Wine's tests record them on Windows (shell32 systray.c: an
//! invalid cbSize works, with the first version's fields; comctl32
//! misc.c: LoadIconMetric's sizes, errors and ordinals).
//!
//! - `messages`: RegisterWindowMessage("TaskbarCreated") is one number in
//!   0xC000..=0xFFFF, the same for the W and A forms and any case.
//! - `icons`: a PNG icon (the executable's RT_ICON 1, Vista's compressed
//!   form) made by CreateIconFromResourceEx at its own 32 x 32 and at
//!   16 x 16 (its colours where the picture has them, transparent
//!   corners); LoadIconMetric's small and large icons are SM_CXSMICON
//!   and SM_CXICON wide (from the module, a standard icon, an .ico file),
//!   LoadIconWithScaleDown any size; its errors (E_INVALIDARG,
//!   HRESULT_FROM_WIN32(ERROR_RESOURCE_TYPE_NOT_FOUND)); ordinals 380
//!   and 381 are the two functions.
//! - `tray` (skipped on a Windows without Explorer's taskbar,
//!   Shell_TrayWnd): NIM_ADD once (a second add fails), NIM_MODIFY,
//!   NIM_SETVERSION, NIM_SETFOCUS of an icon there (not of one that is
//!   not), Shell_NotifyIconGetRect (a rectangle; a failure for an unknown
//!   icon), a balloon (NIF_INFO), NIM_DELETE with cbSize 3, the ANSI
//!   form, an icon named by its GUID.
//! - On gala only (64-bit: `gala tray` and `gala tray click`, the
//!   desktop's named actions): the icon is listed with its tip, a click
//!   is WM_LBUTTONDOWN, WM_LBUTTONUP and NIN_SELECT, a right click
//!   WM_RBUTTONDOWN, WM_RBUTTONUP and WM_CONTEXTMENU, each in version 4's
//!   layout (wParam the point, the icon's middle; lParam the event and
//!   the id); NIN_BALLOONSHOW came with the balloon. With the argument
//!   `screen` the test then waits for a real click on the icon (xtask's
//!   screen check: `tray_test: click the icon` on the serial log).
//!
//! Prints `tray_test: ok` (`wow64_tray_test` for the x86 build), or
//! `<name>: check N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicUsize, Ordering};

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "tray_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_tray_test";

type Handle = usize;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetModuleHandleW(name: *const u16) -> Handle;
    fn LoadLibraryW(name: *const u16) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> usize;
    fn Sleep(ms: u32);
    fn GetTickCount64() -> u64;
    fn GetTempPathW(len: u32, buf: *mut u16) -> u32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: usize,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CloseHandle(h: Handle) -> i32;
    fn DeleteFileW(name: *const u16) -> i32;
    fn QueryPerformanceCounter(count: *mut i64) -> i32;
    fn GetCommandLineW() -> *const u16;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "user32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "user32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn RegisterWindowMessageW(name: *const u16) -> u32;
    fn RegisterWindowMessageA(name: *const u8) -> u32;
    fn RegisterClassW(class: *const WndClass) -> u16;
    #[allow(clippy::too_many_arguments)]
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Handle,
        menu: Handle,
        instance: Handle,
        param: usize,
    ) -> Handle;
    fn DestroyWindow(hwnd: Handle) -> i32;
    fn DefWindowProcW(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn PeekMessageW(msg: *mut Msg, hwnd: Handle, min: u32, max: u32, remove: u32) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn FindWindowW(class: *const u16, title: *const u16) -> Handle;
    fn DestroyIcon(icon: Handle) -> i32;
    fn GetIconInfo(icon: Handle, info: *mut IconInfo) -> i32;
    fn CreateIconFromResourceEx(
        bits: *const u8,
        size: u32,
        icon: i32,
        version: u32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> Handle;
    fn GetSystemMetrics(index: i32) -> i32;
    fn GetDC(hwnd: Handle) -> Handle;
    fn ReleaseDC(hwnd: Handle, dc: Handle) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "gdi32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "gdi32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetObjectW(object: Handle, size: i32, out: *mut Bitmap) -> i32;
    fn GetDIBits(
        dc: Handle,
        bitmap: Handle,
        start: u32,
        lines: u32,
        bits: *mut u32,
        info: *mut u8,
        usage: u32,
    ) -> i32;
    fn DeleteObject(object: Handle) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "shell32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "shell32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn Shell_NotifyIconW(message: u32, data: *const NotifyW) -> i32;
    fn Shell_NotifyIconA(message: u32, data: *const NotifyA) -> i32;
    fn Shell_NotifyIconGetRect(id: *const Identifier, rect: *mut Rect) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "comctl32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "comctl32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn LoadIconMetric(module: Handle, name: *const u16, metric: i32, out: *mut Handle) -> i32;
    fn LoadIconWithScaleDown(
        module: Handle,
        name: *const u16,
        cx: i32,
        cy: i32,
        out: *mut Handle,
    ) -> i32;
}

#[repr(C)]
struct WndClass {
    style: u32,
    proc_: extern "system" fn(Handle, u32, usize, isize) -> isize,
    cls_extra: i32,
    wnd_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu: *const u16,
    class: *const u16,
}

#[repr(C)]
struct Msg {
    hwnd: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: [i32; 2],
    private: u32,
}

#[repr(C)]
struct IconInfo {
    icon: i32,
    x_hotspot: u32,
    y_hotspot: u32,
    mask: Handle,
    color: Handle,
}

#[repr(C)]
struct Bitmap {
    kind: i32,
    width: i32,
    height: i32,
    width_bytes: i32,
    planes: u16,
    bits_pixel: u16,
    bits: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// NOTIFYICONDATAW.
#[repr(C)]
#[derive(Clone, Copy)]
struct NotifyW {
    cb_size: u32,
    hwnd: Handle,
    uid: u32,
    flags: u32,
    callback: u32,
    icon: Handle,
    tip: [u16; 128],
    state: u32,
    state_mask: u32,
    info: [u16; 256],
    version: u32,
    info_title: [u16; 64],
    info_flags: u32,
    guid: [u32; 4],
    balloon_icon: Handle,
}

/// NOTIFYICONDATAA.
#[repr(C)]
#[derive(Clone, Copy)]
struct NotifyA {
    cb_size: u32,
    hwnd: Handle,
    uid: u32,
    flags: u32,
    callback: u32,
    icon: Handle,
    tip: [u8; 128],
    state: u32,
    state_mask: u32,
    info: [u8; 256],
    version: u32,
    info_title: [u8; 64],
    info_flags: u32,
    guid: [u32; 4],
    balloon_icon: Handle,
}

/// NOTIFYICONIDENTIFIER.
#[repr(C)]
struct Identifier {
    cb_size: u32,
    hwnd: Handle,
    uid: u32,
    guid: [u32; 4],
}

const NIM_ADD: u32 = 0;
const NIM_MODIFY: u32 = 1;
const NIM_DELETE: u32 = 2;
const NIM_SETFOCUS: u32 = 3;
const NIM_SETVERSION: u32 = 4;
const NIF_MESSAGE: u32 = 0x01;
const NIF_ICON: u32 = 0x02;
const NIF_TIP: u32 = 0x04;
const NIF_INFO: u32 = 0x10;
const NIF_GUID: u32 = 0x20;
const NIIF_INFO: u32 = 0x01;
const NIIF_NOSOUND: u32 = 0x10;
const NOTIFYICON_VERSION_4: u32 = 4;
/// NOTIFYICONDATAW_V1_SIZE, NOTIFYICONDATAA_V1_SIZE.
const W_V1: u32 = (core::mem::offset_of!(NotifyW, tip) + 2 * 64) as u32;
const A_V1: u32 = (core::mem::offset_of!(NotifyA, tip) + 64) as u32;
const CALLBACK: u32 = 0x8000 + 0x31; // WM_APP + 0x31
#[cfg_attr(target_arch = "x86", allow(dead_code))]
const WM_LBUTTONDOWN: usize = 0x201;
#[cfg_attr(target_arch = "x86", allow(dead_code))]
const WM_LBUTTONUP: usize = 0x202;
#[cfg_attr(target_arch = "x86", allow(dead_code))]
const WM_RBUTTONDOWN: usize = 0x204;
#[cfg_attr(target_arch = "x86", allow(dead_code))]
const WM_RBUTTONUP: usize = 0x205;
#[cfg_attr(target_arch = "x86", allow(dead_code))]
const WM_CONTEXTMENU: usize = 0x7b;
const NIN_SELECT: usize = 0x400;
const NIN_BALLOONSHOW: usize = 0x402;

const S_OK: i32 = 0;
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
const E_RESOURCE_TYPE_NOT_FOUND: i32 = 0x8007_0715u32 as i32;
const LIM_SMALL: i32 = 0;
const LIM_LARGE: i32 = 1;
const SM_CXICON: i32 = 11;
const SM_CYICON: i32 = 12;
const SM_CXSMICON: i32 = 49;
const SM_CYSMICON: i32 = 50;
const IDI_APPLICATION: usize = 32512;

static PNG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/icon.png"));

// ---- Output ---------------------------------------------------------------

struct Line {
    buf: [u8; 300],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 300], n: 0 }
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn dec(self, v: u64) -> Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&d[i..])
    }

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 18];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..16 {
            d[2 + i] = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
    }

    fn print(self) {
        let mut written = 0;
        // SAFETY: our buffer; the standard output handle.
        unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                self.buf.as_ptr(),
                self.n as u32,
                &mut written,
                0,
            )
        };
    }
}

fn say(what: &[u8]) {
    Line::new().s(NAME.as_bytes()).s(b": ").s(what).s(b"\r\n").print();
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    #[track_caller]
    fn hr(&mut self, got: i32, want: i32) -> Result<(), u32> {
        self.ok(got == want, got as u32 as u64)
    }
}

/// A NUL-terminated UTF-16 copy of ASCII `s`.
struct W([u16; 300]);

impl W {
    fn new(s: &str) -> W {
        let mut w = [0u16; 300];
        for (d, b) in w.iter_mut().zip(s.bytes()) {
            *d = b.into();
        }
        W(w)
    }

    fn ptr(&self) -> *const u16 {
        self.0.as_ptr()
    }
}

fn fill(dst: &mut [u16], s: &str) {
    for (d, b) in dst.iter_mut().zip(s.bytes()) {
        *d = b.into();
    }
}

// ---- The window that gets the callbacks ----------------------------------

/// The callback messages the window got: (wParam, lParam) each.
static GOT: [[AtomicUsize; 2]; 64] = [const { [AtomicUsize::new(0), AtomicUsize::new(0)] }; 64];
static COUNT: AtomicUsize = AtomicUsize::new(0);

extern "system" fn proc_(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> isize {
    if msg == CALLBACK {
        let n = COUNT.load(Ordering::Relaxed);
        if n < GOT.len() {
            GOT[n][0].store(wparam, Ordering::Relaxed);
            GOT[n][1].store(lparam as usize, Ordering::Relaxed);
            COUNT.store(n + 1, Ordering::Relaxed);
        }
        return 0;
    }
    // SAFETY: the default handling.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Handles messages until the window has `n` callbacks (or `ms` passed:
/// a generous give-up, not a measurement).
fn pump_until(n: usize, ms: u64) -> bool {
    // SAFETY: plain calls with our MSG.
    unsafe {
        let end = GetTickCount64() + ms;
        let mut msg: Msg = core::mem::zeroed();
        loop {
            while PeekMessageW(&mut msg, 0, 0, 0, 1) != 0 {
                DispatchMessageW(&msg);
            }
            if COUNT.load(Ordering::Relaxed) >= n {
                return true;
            }
            if GetTickCount64() > end {
                return false;
            }
            Sleep(10);
        }
    }
}

fn got(i: usize) -> (usize, usize) {
    (GOT[i][0].load(Ordering::Relaxed), GOT[i][1].load(Ordering::Relaxed))
}

// ---- Icons ------------------------------------------------------------------

/// An icon's colour bitmap: (width, height, bits per pixel).
fn icon_size(icon: Handle) -> (i32, i32, u16) {
    // SAFETY: plain calls; the bitmaps GetIconInfo copied are deleted.
    unsafe {
        let mut ii: IconInfo = core::mem::zeroed();
        if GetIconInfo(icon, &mut ii) == 0 {
            return (0, 0, 0);
        }
        let mut bm: Bitmap = core::mem::zeroed();
        let n = GetObjectW(ii.color, size_of::<Bitmap>() as i32, &mut bm);
        DeleteObject(ii.color);
        DeleteObject(ii.mask);
        if n == 0 { (0, 0, 0) } else { (bm.width, bm.height, bm.bits_pixel) }
    }
}

/// Pixel (x, y) of an icon's colour bitmap (0xAARRGGBB), read as a
/// top-down 32-bit DIB of `side` x `side`.
fn icon_pixel(icon: Handle, side: i32, x: i32, y: i32) -> u32 {
    let mut px = [0u32; 32 * 32];
    if side > 32 {
        return 0xdead;
    }
    // SAFETY: plain calls with our buffers.
    unsafe {
        let mut ii: IconInfo = core::mem::zeroed();
        if GetIconInfo(icon, &mut ii) == 0 {
            return 0xdead;
        }
        let mut bih = [0u8; 44];
        bih[0..4].copy_from_slice(&40u32.to_le_bytes());
        bih[4..8].copy_from_slice(&side.to_le_bytes());
        bih[8..12].copy_from_slice(&(-side).to_le_bytes());
        bih[12..14].copy_from_slice(&1u16.to_le_bytes());
        bih[14..16].copy_from_slice(&32u16.to_le_bytes());
        let dc = GetDC(0);
        let lines = GetDIBits(dc, ii.color, 0, side as u32, px.as_mut_ptr(), bih.as_mut_ptr(), 0);
        ReleaseDC(0, dc);
        DeleteObject(ii.color);
        DeleteObject(ii.mask);
        if lines != side {
            return 0xdead;
        }
    }
    px[(y * side + x) as usize]
}

fn icons(c: &mut Checks) -> Result<Handle, u32> {
    say(b"icons");
    // SAFETY: plain calls; icons made here are destroyed.
    unsafe {
        let me = GetModuleHandleW(core::ptr::null());
        // The PNG form, at its own size and at 16 x 16.
        let big = CreateIconFromResourceEx(PNG.as_ptr(), PNG.len() as u32, 1, 0x30000, 0, 0, 0);
        c.ok(big != 0, 0)?;
        let (w, h, bpp) = icon_size(big);
        c.eq((w as u64) << 32 | h as u64, 32 << 32 | 32)?;
        c.eq(bpp.into(), 32)?;
        c.eq(icon_pixel(big, 32, 6, 6).into(), 0xff20_80f0)?;
        c.eq(icon_pixel(big, 32, 16, 16).into(), 0xffff_ffff)?;
        c.eq((icon_pixel(big, 32, 0, 0) >> 24).into(), 0)?;
        DestroyIcon(big);
        let small = CreateIconFromResourceEx(PNG.as_ptr(), PNG.len() as u32, 1, 0x30000, 16, 16, 0);
        c.ok(small != 0, 0)?;
        let (w, h, _) = icon_size(small);
        c.eq((w as u64) << 32 | h as u64, 16 << 32 | 16)?;
        c.eq(icon_pixel(small, 16, 3, 3).into(), 0xff20_80f0)?;
        c.eq(icon_pixel(small, 16, 8, 8).into(), 0xffff_ffff)?;
        DestroyIcon(small);

        // LoadIconMetric and LoadIconWithScaleDown.
        let sm = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
        let lg = (GetSystemMetrics(SM_CXICON), GetSystemMetrics(SM_CYICON));
        let mut icon: Handle = 0x1234;
        c.hr(LoadIconMetric(0, IDI_APPLICATION as *const u16, 0x100, &mut icon), E_INVALIDARG)?;
        c.eq(icon as u64, 0)?;
        icon = 0x1234;
        c.hr(LoadIconMetric(0, core::ptr::null(), LIM_LARGE, &mut icon), E_INVALIDARG)?;
        c.eq(icon as u64, 0)?;
        icon = 0x1234;
        c.hr(LoadIconWithScaleDown(0, core::ptr::null(), 32, 32, &mut icon), E_INVALIDARG)?;
        c.eq(icon as u64, 0)?;
        let missing = W::new("Nonexisting");
        c.hr(LoadIconMetric(me, missing.ptr(), LIM_LARGE, &mut icon), E_RESOURCE_TYPE_NOT_FOUND)?;
        for (metric, want) in [(LIM_SMALL, sm), (LIM_LARGE, lg)] {
            c.hr(LoadIconMetric(0, IDI_APPLICATION as *const u16, metric, &mut icon), S_OK)?;
            let (w, h, _) = icon_size(icon);
            c.eq((w as u64) << 32 | h as u64, (want.0 as u64) << 32 | want.1 as u64)?;
            DestroyIcon(icon);
            c.hr(
                LoadIconMetric(me, core::ptr::without_provenance::<u16>(1), metric, &mut icon),
                S_OK,
            )?;
            let (w, h, _) = icon_size(icon);
            c.eq((w as u64) << 32 | h as u64, (want.0 as u64) << 32 | want.1 as u64)?;
            DestroyIcon(icon);
        }
        c.hr(
            LoadIconWithScaleDown(me, core::ptr::without_provenance::<u16>(1), 24, 24, &mut icon),
            S_OK,
        )?;
        let (w, h, _) = icon_size(icon);
        c.eq((w as u64) << 32 | h as u64, 24 << 32 | 24)?;
        DestroyIcon(icon);
        c.hr(LoadIconWithScaleDown(0, IDI_APPLICATION as *const u16, 42, 42, &mut icon), S_OK)?;
        let (w, h, _) = icon_size(icon);
        c.eq((w as u64) << 32 | h as u64, 42 << 32 | 42)?;
        DestroyIcon(icon);

        // An .ico file: ICONDIR, one entry, the PNG.
        let mut path = [0u16; 300];
        let n = GetTempPathW(260, path.as_mut_ptr()) as usize;
        c.ok(n > 0 && n < 260, n as u64)?;
        fill(&mut path[n..], "tray_test.ico");
        let mut ico = [0u8; 22 + 8192];
        c.ok(PNG.len() <= 8192, PNG.len() as u64)?;
        ico[..6].copy_from_slice(&[0, 0, 1, 0, 1, 0]);
        ico[6..14].copy_from_slice(&[32, 32, 0, 0, 1, 0, 32, 0]);
        ico[14..18].copy_from_slice(&(PNG.len() as u32).to_le_bytes());
        ico[18..22].copy_from_slice(&22u32.to_le_bytes());
        ico[22..22 + PNG.len()].copy_from_slice(PNG);
        let f = CreateFileW(path.as_ptr(), 0x4000_0000, 0, 0, 2, 0x80, 0);
        c.ok(f != usize::MAX, 0)?;
        let mut written = 0;
        WriteFile(f, ico.as_ptr(), (22 + PNG.len()) as u32, &mut written, 0);
        CloseHandle(f);
        c.hr(LoadIconMetric(0, path.as_ptr(), LIM_SMALL, &mut icon), S_OK)?;
        let (w, h, _) = icon_size(icon);
        c.eq((w as u64) << 32 | h as u64, (sm.0 as u64) << 32 | sm.1 as u64)?;
        DestroyIcon(icon);
        DeleteFileW(path.as_ptr());
        fill(&mut path[n..], "tray_test_none.ico");
        c.hr(LoadIconMetric(0, path.as_ptr(), LIM_LARGE, &mut icon), E_RESOURCE_TYPE_NOT_FOUND)?;

        // The ordinals.
        let comctl = LoadLibraryW(W::new("comctl32.dll").ptr());
        c.ok(comctl != 0, 0)?;
        let by_name = GetProcAddress(comctl, c"LoadIconMetric".as_ptr().cast());
        c.ok(by_name != 0 && GetProcAddress(comctl, 380 as *const u8) == by_name, by_name as u64)?;
        let by_name = GetProcAddress(comctl, c"LoadIconWithScaleDown".as_ptr().cast());
        c.ok(by_name != 0 && GetProcAddress(comctl, 381 as *const u8) == by_name, by_name as u64)?;

        // The tray's icon: the program's own, small.
        c.hr(
            LoadIconMetric(me, core::ptr::without_provenance::<u16>(1), LIM_SMALL, &mut icon),
            S_OK,
        )?;
        Ok(icon)
    }
}

// ---- gala's named actions (64-bit) ----------------------------------------

#[cfg(target_arch = "x86_64")]
mod cli {
    use super::{CloseHandle, GetStdHandle, Handle, W};

    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn CreatePipe(read: *mut Handle, write: *mut Handle, sa: *const usize, size: u32) -> i32;
        fn SetHandleInformation(h: Handle, mask: u32, flags: u32) -> i32;
        #[allow(clippy::too_many_arguments)]
        fn CreateProcessW(
            app: *const u16,
            cmd: *mut u16,
            pa: usize,
            ta: usize,
            inherit: i32,
            flags: u32,
            env: usize,
            dir: *const u16,
            startup: *const u8,
            info: *mut [usize; 3],
        ) -> i32;
        fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: usize) -> i32;
        fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
        fn GetExitCodeProcess(h: Handle, code: *mut u32) -> i32;
    }

    pub struct Output {
        pub b: [u8; 4096],
        pub n: usize,
    }

    impl Output {
        pub fn has(&self, what: &[u8]) -> bool {
            self.b[..self.n].windows(what.len()).any(|w| w == what)
        }
    }

    /// Runs `C:\gala\gala.exe <args>`: its exit code and output.
    pub fn gala(args: &[u8]) -> (u32, Output) {
        let mut o = Output { b: [0; 4096], n: 0 };
        let mut line = [0u16; 300];
        for (d, &b) in line.iter_mut().zip(b"gala.exe ".iter().chain(args)) {
            *d = b.into();
        }
        let exe = W::new("C:\\gala\\gala.exe");
        let sa = [24usize, 0, 1];
        let (mut r, mut w) = (0, 0);
        // SAFETY: plain calls with our buffers and handles.
        unsafe {
            if CreatePipe(&mut r, &mut w, sa.as_ptr(), 0) == 0 {
                return (0xffff_0000, o);
            }
            SetHandleInformation(r, 1, 0);
            let mut startup = [0u8; 104];
            startup[0] = 104;
            startup[60..64].copy_from_slice(&0x100u32.to_le_bytes()); // STARTF_USESTDHANDLES
            startup[80..88].copy_from_slice(&(GetStdHandle(-10i32 as u32) as u64).to_le_bytes());
            startup[88..96].copy_from_slice(&(w as u64).to_le_bytes());
            startup[96..104].copy_from_slice(&(w as u64).to_le_bytes());
            let mut info = [0usize; 3];
            let ok = CreateProcessW(
                exe.ptr(),
                line.as_mut_ptr(),
                0,
                0,
                1,
                0,
                0,
                core::ptr::null(),
                startup.as_ptr(),
                &mut info,
            );
            CloseHandle(w);
            if ok == 0 {
                CloseHandle(r);
                return (0xfffe_0000, o);
            }
            let mut got = 0;
            while o.n < o.b.len()
                && ReadFile(r, o.b.as_mut_ptr().add(o.n), (o.b.len() - o.n) as u32, &mut got, 0)
                    != 0
                && got > 0
            {
                o.n += got as usize;
            }
            CloseHandle(r);
            WaitForSingleObject(info[0], 60_000);
            let mut code = 0xdead_0000;
            GetExitCodeProcess(info[0], &mut code);
            CloseHandle(info[0]);
            CloseHandle(info[1]);
            (code, o)
        }
    }

    /// `hwnd` in hexadecimal, as `gala tray click` takes it.
    pub fn hex(hwnd: Handle, out: &mut [u8; 18]) -> &[u8] {
        out[0] = b'0';
        out[1] = b'x';
        for i in 0..16 {
            out[2 + i] = b"0123456789abcdef"[(hwnd >> (60 - 4 * i)) & 0xf];
        }
        &out[..]
    }
}

/// The checks on gala's named actions: the listing, a click and a right
/// click through `gala tray click`, each callback in version 4's layout.
#[cfg(target_arch = "x86_64")]
fn named_actions(c: &mut Checks, hwnd: Handle, middle: usize) -> Result<(), u32> {
    say(b"gala tray");
    let (code, out) = cli::gala(b"tray");
    c.eq(code.into(), 0)?;
    c.ok(out.has(b"tray_test icon"), out.n as u64)?;
    let mut h = [0u8; 18];
    let mut args = [0u8; 64];
    for (button, events) in [
        (&b""[..], [WM_LBUTTONDOWN, WM_LBUTTONUP, NIN_SELECT]),
        (&b" right"[..], [WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CONTEXTMENU]),
    ] {
        let mut n = 0;
        for &b in b"tray click ".iter().chain(cli::hex(hwnd, &mut h)).chain(b" 1").chain(button) {
            args[n] = b;
            n += 1;
        }
        let before = COUNT.load(Ordering::Relaxed);
        let (code, _) = cli::gala(&args[..n]);
        c.eq(code.into(), 0)?;
        c.ok(pump_until(before + 3, 60_000), COUNT.load(Ordering::Relaxed) as u64)?;
        for (k, event) in events.into_iter().enumerate() {
            let (wp, lp) = got(before + k);
            c.eq(wp as u64, middle as u64)?;
            c.eq(lp as u64, (event | 1 << 16) as u64)?;
        }
    }
    let mut n = 0;
    for &b in b"tray click ".iter().chain(cli::hex(hwnd, &mut h)).chain(b" 77") {
        args[n] = b;
        n += 1;
    }
    let (code, _) = cli::gala(&args[..n]);
    c.eq(code.into(), 2)?; // not found
    Ok(())
}

#[cfg(target_arch = "x86")]
fn named_actions(_: &mut Checks, _: Handle, _: usize) -> Result<(), u32> {
    Ok(())
}

/// Whether the command line asks for the screen part (`screen`).
fn screen_arg() -> bool {
    // SAFETY: the process's command line, NUL-terminated.
    unsafe {
        let p = GetCommandLineW();
        let mut i = 0;
        let want: [u16; 6] = [115, 99, 114, 101, 101, 110];
        loop {
            if *p.add(i) == 0 {
                return false;
            }
            if (0..6).all(|k| *p.add(i + k) == want[k]) {
                return true;
            }
            i += 1;
        }
    }
}

fn tray(c: &mut Checks, icon: Handle) -> Result<(), u32> {
    say(b"tray");
    let gala = ntrt::on_gala();
    // SAFETY: plain calls with our structures; the window and icons made
    // here are destroyed.
    unsafe {
        if !gala && FindWindowW(W::new("Shell_TrayWnd").ptr(), core::ptr::null()) == 0 {
            say(b"tray skipped (no notification area: Explorer's taskbar is not running)");
            return Ok(());
        }
        let class = W::new("tray_test window");
        let wc = WndClass {
            style: 0,
            proc_,
            cls_extra: 0,
            wnd_extra: 0,
            instance: GetModuleHandleW(core::ptr::null()),
            icon: 0,
            cursor: 0,
            background: 0,
            menu: core::ptr::null(),
            class: class.ptr(),
        };
        c.ok(RegisterClassW(&wc) != 0, 0)?;
        let hwnd =
            CreateWindowExW(0, class.ptr(), class.ptr(), 0, 0, 0, 100, 100, 0, 0, wc.instance, 0);
        c.ok(hwnd != 0, 0)?;

        let mut d: NotifyW = core::mem::zeroed();
        d.cb_size = W_V1;
        d.hwnd = hwnd;
        d.uid = 1;
        d.flags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        d.callback = CALLBACK;
        d.icon = icon;
        fill(&mut d.tip, "tray_test");
        c.eq(Shell_NotifyIconW(NIM_ADD, &d) as u64, 1)?;
        c.eq(Shell_NotifyIconW(NIM_ADD, &d) as u64, 0)?;
        d.cb_size = size_of::<NotifyW>() as u32;
        d.flags = NIF_TIP;
        fill(&mut d.tip, "tray_test icon");
        c.eq(Shell_NotifyIconW(NIM_MODIFY, &d) as u64, 1)?;
        d.uid = 99;
        c.eq(Shell_NotifyIconW(NIM_MODIFY, &d) as u64, 0)?;
        d.uid = 1;
        d.version = NOTIFYICON_VERSION_4;
        c.eq(Shell_NotifyIconW(NIM_SETVERSION, &d) as u64, 1)?;

        let id = Identifier { cb_size: size_of::<Identifier>() as u32, hwnd, uid: 1, guid: [0; 4] };
        let mut r = Rect::default();
        c.hr(Shell_NotifyIconGetRect(&id, &mut r), S_OK)?;
        c.ok(r.right > r.left && r.bottom > r.top, r.left as u64)?;
        let none = Identifier { uid: 98, ..id };
        let mut nr = Rect::default();
        c.ok(Shell_NotifyIconGetRect(&none, &mut nr) < 0, 0)?;
        let middle = ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
        if gala {
            // shasha's taskbar: the bottom 48 rows; the icon in them.
            let (sw, sh) = (GetSystemMetrics(0), GetSystemMetrics(1));
            c.ok(r.top >= sh - 48 && r.bottom <= sh && r.right <= sw, r.top as u64)?;
            Line::new()
                .s(NAME.as_bytes())
                .s(b": icon at ")
                .dec(middle.0 as u64)
                .s(b",")
                .dec(middle.1 as u64)
                .s(b"\r\n")
                .print();
        }
        c.eq(Shell_NotifyIconW(NIM_SETFOCUS, &d) as u64, 1)?;
        d.uid = 99;
        c.eq(Shell_NotifyIconW(NIM_SETFOCUS, &d) as u64, 0)?;
        d.uid = 1;

        if gala {
            let packed = (middle.0 as u16 as usize) | (middle.1 as u16 as usize) << 16;
            named_actions(c, hwnd, packed)?;
        }

        // A balloon (Windows 11 shows it as a notification).
        let before = COUNT.load(Ordering::Relaxed);
        d.flags = NIF_INFO;
        d.info_flags = NIIF_INFO | NIIF_NOSOUND;
        fill(&mut d.info_title, "gala");
        fill(&mut d.info, "The notification area works: this is a balloon.");
        c.eq(Shell_NotifyIconW(NIM_MODIFY, &d) as u64, 1)?;
        if gala {
            c.ok(pump_until(before + 1, 30_000), 0)?;
            let (_, lp) = got(before);
            c.eq((lp & 0xffff) as u64, NIN_BALLOONSHOW as u64)?;
            if screen_arg() {
                // xtask moves the mouse to the icon and clicks it.
                let before = COUNT.load(Ordering::Relaxed);
                say(b"click the icon");
                let mut clicked = false;
                let end = GetTickCount64() + 300_000;
                while !clicked && GetTickCount64() < end {
                    pump_until(COUNT.load(Ordering::Relaxed) + 1, 1_000);
                    let n = COUNT.load(Ordering::Relaxed);
                    clicked = (before..n).any(|k| got(k).1 & 0xffff == NIN_SELECT);
                }
                c.ok(clicked, COUNT.load(Ordering::Relaxed) as u64)?;
                say(b"clicked");
            }
        }

        // NIM_DELETE with an invalid cbSize works (the first version's
        // fields); then the icon is gone.
        d.cb_size = 3;
        c.eq(Shell_NotifyIconW(NIM_DELETE, &d) as u64, 1)?;
        d.cb_size = size_of::<NotifyW>() as u32;
        c.eq(Shell_NotifyIconW(NIM_DELETE, &d) as u64, 0)?;

        // The ANSI form.
        let mut a: NotifyA = core::mem::zeroed();
        a.cb_size = A_V1;
        a.hwnd = hwnd;
        a.uid = 2;
        a.flags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        a.callback = CALLBACK;
        a.icon = icon;
        a.tip[..10].copy_from_slice(b"tray_test2");
        c.eq(Shell_NotifyIconA(NIM_ADD, &a) as u64, 1)?;
        a.cb_size = 3;
        c.eq(Shell_NotifyIconA(NIM_DELETE, &a) as u64, 1)?;
        a.cb_size = size_of::<NotifyA>() as u32;
        c.eq(Shell_NotifyIconA(NIM_DELETE, &a) as u64, 0)?;

        // An icon named by its GUID (a new one each run: Windows ties a
        // GUID to the program's path).
        let mut t = 0i64;
        QueryPerformanceCounter(&mut t);
        let mut g: NotifyW = core::mem::zeroed();
        g.cb_size = size_of::<NotifyW>() as u32;
        g.hwnd = hwnd;
        g.flags = NIF_ICON | NIF_GUID;
        g.icon = icon;
        g.guid = [0x7a1c_0000 ^ t as u32, (t >> 32) as u32 | 0x4000_0000, 0x6761_6c61, 0x7472_6179];
        c.eq(Shell_NotifyIconW(NIM_ADD, &g) as u64, 1)?;
        let gid =
            Identifier { cb_size: size_of::<Identifier>() as u32, hwnd: 0, uid: 0, guid: g.guid };
        c.hr(Shell_NotifyIconGetRect(&gid, &mut r), S_OK)?;
        c.eq(Shell_NotifyIconW(NIM_DELETE, &g) as u64, 1)?;
        c.eq(Shell_NotifyIconW(NIM_DELETE, &g) as u64, 0)?;
        DestroyWindow(hwnd);
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    say(b"messages");
    // SAFETY: plain calls with our strings.
    unsafe {
        let m = RegisterWindowMessageW(W::new("TaskbarCreated").ptr());
        c.ok((0xc000..=0xffff).contains(&m), m.into())?;
        c.eq(RegisterWindowMessageA(c"TaskbarCreated".as_ptr().cast()).into(), m.into())?;
        c.eq(RegisterWindowMessageW(W::new("taskbarcreated").ptr()).into(), m.into())?;
    }
    let icon = icons(&mut c)?;
    tray(&mut c, icon)?;
    // SAFETY: our icon.
    unsafe { DestroyIcon(icon) };
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            say(b"ok");
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
