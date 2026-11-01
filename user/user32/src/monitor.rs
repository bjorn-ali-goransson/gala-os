//! The display: one monitor, the desktop's screen (GalaScreenInfo: its
//! size and the work area above the taskbar; without a screen, as in
//! gala-um, 1280 x 800 with no taskbar). HMONITOR is a constant.
//! EnumDisplayMonitors, MonitorFromWindow/Point/Rect, GetMonitorInfoW/A
//! and the screen metrics come from here.

use crate::window::Hwnd;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaScreenInfo(info: *mut [i32; 8]) -> u32;
}

pub const HMONITOR: usize = 0x0001_0001;
const MONITOR_DEFAULTTONULL: u32 = 0;
const MONITORINFOF_PRIMARY: u32 = 1;

pub type Rect = [i32; 4];

/// The screen and the work area.
pub fn screen() -> (Rect, Rect) {
    let mut info = [0i32; 8];
    // SAFETY: our buffer of eight i32.
    if unsafe { GalaScreenInfo(&mut info) } == 0 {
        ([info[0], info[1], info[2], info[3]], [info[4], info[5], info[6], info[7]])
    } else {
        ([0, 0, 1280, 800], [0, 0, 1280, 800])
    }
}

fn intersects(a: &Rect, b: &Rect) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// The monitor for an area: the one monitor if they meet, else by `flags`
/// (MONITOR_DEFAULTTONULL: none; to the primary or the nearest: it).
fn for_rect(r: &Rect, flags: u32) -> usize {
    let (s, _) = screen();
    if intersects(r, &s) || flags != MONITOR_DEFAULTTONULL { HMONITOR } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "system" fn MonitorFromRect(rect: *const Rect, flags: u32) -> usize {
    if rect.is_null() {
        return 0;
    }
    // SAFETY: the caller's RECT.
    for_rect(unsafe { &*rect }, flags)
}

/// MonitorFromPoint(POINT by value: x in the low 32 bits, y in the high).
#[unsafe(no_mangle)]
pub extern "system" fn MonitorFromPoint(point: u64, flags: u32) -> usize {
    let (x, y) = (point as u32 as i32, (point >> 32) as u32 as i32);
    for_rect(&[x, y, x + 1, y + 1], flags)
}

#[unsafe(no_mangle)]
pub extern "system" fn MonitorFromWindow(hwnd: Hwnd, flags: u32) -> usize {
    let mut r: Rect = [0; 4];
    if crate::window::GetWindowRect(hwnd, &mut r) == 0 {
        // Not a window: the default (Windows answers for any handle).
        return if flags != MONITOR_DEFAULTTONULL { HMONITOR } else { 0 };
    }
    for_rect(&r, flags)
}

/// GetMonitorInfoW(monitor, MONITORINFO or MONITORINFOEXW): by cbSize (40
/// or 104); the device name is \\.\DISPLAY1.
#[unsafe(no_mangle)]
pub extern "system" fn GetMonitorInfoW(monitor: usize, info: *mut u8) -> i32 {
    fill(monitor, info, true)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetMonitorInfoA(monitor: usize, info: *mut u8) -> i32 {
    fill(monitor, info, false)
}

fn fill(monitor: usize, info: *mut u8, wide: bool) -> i32 {
    if monitor != HMONITOR || info.is_null() {
        return 0;
    }
    // SAFETY: the caller's structure, cbSize first.
    let size = unsafe { info.cast::<u32>().read_unaligned() };
    let ex = if wide { 104 } else { 72 };
    if size != 40 && size != ex {
        return 0;
    }
    let (s, w) = screen();
    // SAFETY: at least 40 bytes, as cbSize says.
    unsafe {
        for (i, v) in s.iter().chain(w.iter()).enumerate() {
            info.add(4 + 4 * i).cast::<i32>().write_unaligned(*v);
        }
        info.add(36).cast::<u32>().write_unaligned(MONITORINFOF_PRIMARY);
        if size == ex {
            let name = b"\\\\.\\DISPLAY1";
            for (i, &c) in name.iter().chain(core::iter::once(&0)).enumerate() {
                if wide {
                    info.add(40 + 2 * i).cast::<u16>().write_unaligned(u16::from(c));
                } else {
                    *info.add(40 + i) = c;
                }
            }
        }
    }
    1
}

type MonitorEnumProc = extern "system" fn(usize, usize, *const Rect, isize) -> i32;

/// EnumDisplayMonitors(hdc, clip, proc, data): the one monitor (if it
/// meets `clip`), with its rectangle; TRUE.
#[unsafe(no_mangle)]
pub extern "system" fn EnumDisplayMonitors(
    _hdc: usize,
    clip: *const Rect,
    proc_: Option<MonitorEnumProc>,
    data: isize,
) -> i32 {
    let Some(p) = proc_ else { return 0 };
    let (s, _) = screen();
    // SAFETY: the caller's RECT, if any.
    let area = if clip.is_null() { s } else { unsafe { *clip } };
    if intersects(&area, &s) {
        p(HMONITOR, 0, &s, data);
    }
    1
}
