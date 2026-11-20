//! A Linux client's windows in the session's window registry (ROADMAP SH
//! step 1, part 2b; ARCHITECTURE "Linux windows in the window
//! registry"): each top-level window the Wayland server makes for a
//! client (Xwayland's for X programs) is registered as a top-level window
//! of the client's process, so it has an HWND as a Windows program's
//! window has. The window calls (FindWindow, EnumWindows, GetWindowText,
//! IsIconic, GetWindowThreadProcessId) answer for it, GalaDesktopAction
//! lists it by that HWND and its verbs reach it, and the server raises
//! the WinEvents user32 raises for its own windows (a taskbar's
//! EVENT_SYSTEM_FOREGROUND, MINIMIZESTART, MINIMIZEEND) when the desktop
//! tells it the window became active or was minimized or restored.
//!
//! The record follows the desktop: its rectangle and state are read back
//! from the desktop's window whenever the server places, sizes or
//! retitles it and when the desktop's notices say it moved or changed
//! state. Popups (menus, tooltips: xdg_popup) are not registered: they
//! are not windows a taskbar or a window list shows. Thread ids are 0:
//! Linux threads have no Windows thread id (GetWindowThreadProcessId's
//! thread is 0; the process id is the Linux process's).

use crate::nt::desktop;
use crate::nt::window::{FLAG_ENABLED, FLAG_ICONIC, FLAG_VISIBLE, FLAG_ZOOMED, Info};
use crate::nt::winstate::{
    GALA_WM_ACTIVE, GALA_WM_MINIMIZED, GALA_WM_PLACE, WM_MOVE, WM_SYSCOMMAND,
};

/// The class a Linux window is registered with (GetClassName).
const CLASS: &str = "GalaLinuxWindow";
/// WS_OVERLAPPEDWINDOW (framed by the desktop) and WS_POPUP (frameless:
/// a client that draws its own frame).
const WS_OVERLAPPEDWINDOW: u32 = 0x00cf_0000;
const WS_POPUP: u32 = 0x8000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
/// The WinEvents raised (Microsoft's numbers), for the window itself.
const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const EVENT_SYSTEM_MINIMIZESTART: u32 = 0x0016;
const EVENT_SYSTEM_MINIMIZEEND: u32 = 0x0017;
const OBJID_WINDOW: i32 = 0;
const CHILDID_SELF: i32 = 0;

/// Registers process `pid`'s new top-level desktop window `id` (framed
/// by the desktop when `decorated`) titled `title`: its HWND, or None
/// (the reason logged).
pub(super) fn register(pid: u64, id: u32, title: &[u16], decorated: bool) -> Option<u64> {
    record(pid, id, Some(title), Some(decorated))
}

/// Process `pid`'s registered window `id` changed: its record follows
/// (its new `title` when given). Nothing for a window that has no record
/// (a popup).
pub(super) fn update(pid: u64, id: u32, title: Option<&[u16]>) -> Option<u64> {
    record(pid, id, title, None)
}

/// Records process `pid`'s desktop window `id`: its place and state as
/// the desktop has them, and `title` when given; registered when `new`
/// says how it is framed, else only updated. Its HWND, or None when the
/// window, its record or the process is gone.
fn record(pid: u64, id: u32, title: Option<&[u16]>, new: Option<bool>) -> Option<u64> {
    let process = crate::nt::process::by_id(pid)?;
    let placed = desktop::with(&process.desk.shell, |d| Ok((d.placement(id, pid), None)));
    let p = placed.ok().flatten()?;
    let windows = &process.desk.windows;
    let pid32 = pid as u32;
    let old = windows.hwnd_of_desktop(pid32, id).and_then(|h| windows.query(h));
    let decorated = match (old, new) {
        (None, None) => return None,
        (_, Some(decorated)) => decorated,
        (Some(_), None) => true,
    };
    let mut info = old.unwrap_or_else(|| {
        let mut info = Info { desktop: id, ..Info::default() };
        for (d, s) in info.class.iter_mut().zip(CLASS.encode_utf16()) {
            *d = s;
        }
        info.class_len = CLASS.len() as u32;
        info.style = if decorated { WS_OVERLAPPEDWINDOW } else { WS_POPUP };
        info
    });
    let (w, h) = (p.w.min(i32::MAX as u32) as i32, p.h.min(i32::MAX as u32) as i32);
    info.rect = [p.x, p.y, p.x.saturating_add(w), p.y.saturating_add(h)];
    info.flags = FLAG_ENABLED
        | if p.visible { FLAG_VISIBLE } else { 0 }
        | if p.minimized { FLAG_ICONIC } else { 0 }
        | if p.maximized { FLAG_ZOOMED } else { 0 };
    info.style = (info.style & !WS_VISIBLE) | if p.visible { WS_VISIBLE } else { 0 };
    if let Some(title) = title {
        let n = title.len().min(info.text.len());
        info.text[..n].copy_from_slice(&title[..n]);
        info.text[n..].fill(0);
        info.text_len = n as u32;
    }
    match old {
        Some(old) => {
            let hwnd = old.hwnd;
            if let Err(e) = windows.update(pid32, info) {
                crate::warn!(
                    "wayland: pid {pid}: window {id}'s record {hwnd:#x} not updated: {e:?}"
                );
            }
            Some(hwnd)
        }
        None => match windows.register(pid32, info) {
            Ok(hwnd) => {
                crate::info!("wayland: pid {pid}'s window {id} is hwnd {hwnd:#x}");
                Some(hwnd)
            }
            Err(e) => {
                // Without its record the window is on the screen but no
                // window call or taskbar sees it.
                crate::warn!("wayland: pid {pid}'s window {id} has no hwnd: {e:?}");
                None
            }
        },
    }
}

/// Process `pid`'s desktop window `id` is gone: so is its record.
pub(super) fn forget(pid: u64, id: u32) {
    let Some(process) = crate::nt::process::by_id(pid) else { return };
    let windows = &process.desk.windows;
    if let Some(hwnd) = windows.hwnd_of_desktop(pid as u32, id) {
        let _ = windows.unregister(pid as u32, hwnd);
        crate::info!("wayland: pid {pid}'s window {id} (hwnd {hwnd:#x}) is gone");
    }
}

/// The desktop's notice `message` (`wparam`) for process `pid`'s window
/// `id`: the record follows the window, and the window's activation,
/// minimize and restore raise the WinEvents user32 raises for its own
/// windows, in the order the desktop sends them (a restore's
/// MINIMIZEEND before its FOREGROUND, as on Windows).
pub(super) fn notice(pid: u64, id: u32, message: u32, wparam: u64, trace: bool) {
    let event = match message {
        GALA_WM_ACTIVE if wparam != 0 => Some(EVENT_SYSTEM_FOREGROUND),
        GALA_WM_MINIMIZED if wparam != 0 => Some(EVENT_SYSTEM_MINIMIZESTART),
        GALA_WM_MINIMIZED => Some(EVENT_SYSTEM_MINIMIZEEND),
        GALA_WM_ACTIVE | GALA_WM_PLACE | WM_MOVE | WM_SYSCOMMAND => None,
        _ => return,
    };
    // Only a registered window (not a popup) is re-recorded.
    let Some(hwnd) = update(pid, id, None) else { return };
    let Some(process) = crate::nt::process::by_id(pid) else { return };
    let Some(event) = event else { return };
    let time = (crate::runtime::monotonic_ns() / 1_000_000) as u32;
    let woken = crate::nt::winevent::raise(
        &process.desk,
        pid as u32,
        0,
        event,
        hwnd,
        OBJID_WINDOW,
        CHILDID_SELF,
        time,
    );
    if trace {
        crate::info!(
            "wayland: pid {pid}: event {event:#x} for hwnd {hwnd:#x} ({woken} hooking process(es))"
        );
    }
}
