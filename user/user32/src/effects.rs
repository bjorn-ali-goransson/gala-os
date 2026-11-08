//! Window effects and shutdown etiquette Delphi's VCL (Inno Setup) asks
//! for: SetLayeredWindowAttributes/GetLayeredWindowAttributes (a
//! WS_EX_LAYERED window's colour key, alpha and LWA_* flags, kept and
//! read back; gala's desktop does not blend yet: recorded),
//! AnimateWindow (the window shown or hidden as documented, with its
//! failure cases; the animation itself is not played: recorded),
//! DisableProcessWindowsGhosting (gala does not ghost hung windows), and
//! ShutdownBlockReasonCreate/Query/Destroy (a top-level window's reason,
//! kept until destroyed; gala never shuts down under a program, so no one
//! is shown it); GetLastActivePopup (the owned popup activated last, which
//! `window::activate` records with its owner) and ShowOwnedPopups.

use crate::window::{Hwnd, ShowWindow, index_of, me, set_last_error, unsupported, window, with};
use alloc::vec::Vec;

const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_WINDOW_HANDLE: u32 = 1400;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const WS_EX_LAYERED: u32 = 0x0008_0000;
const LWA_COLORKEY: u32 = 1;
const LWA_ALPHA: u32 = 2;

/// SetLayeredWindowAttributes(hwnd, key, alpha, flags): kept for a
/// WS_EX_LAYERED window (GetLayeredWindowAttributes reads them back);
/// FALSE with ERROR_INVALID_PARAMETER for a window that is not layered.
#[unsafe(no_mangle)]
pub extern "system" fn SetLayeredWindowAttributes(
    hwnd: Hwnd,
    key: u32,
    alpha: u8,
    flags: u32,
) -> i32 {
    let Some(i) = index_of(hwnd) else {
        set_last_error(ERROR_INVALID_WINDOW_HANDLE);
        return 0;
    };
    let ok = with(|s| {
        let w = &mut s.windows[i];
        if !w.used || w.ex_style & WS_EX_LAYERED == 0 {
            return false;
        }
        w.layered = Some((key, alpha, flags & (LWA_COLORKEY | LWA_ALPHA)));
        true
    });
    if !ok {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    if (flags & LWA_ALPHA != 0 && alpha != 255) || flags & LWA_COLORKEY != 0 {
        unsupported(b"user32.dll!SetLayeredWindowAttributes(not blended)", u64::from(flags));
    }
    1
}

/// GetLayeredWindowAttributes(hwnd, *key, *alpha, *flags): what
/// SetLayeredWindowAttributes gave a window that is still layered; FALSE
/// otherwise (the pointers may be NULL).
#[unsafe(no_mangle)]
pub extern "system" fn GetLayeredWindowAttributes(
    hwnd: Hwnd,
    key: *mut u32,
    alpha: *mut u8,
    flags: *mut u32,
) -> i32 {
    let Some(w) = window(hwnd) else {
        set_last_error(ERROR_INVALID_WINDOW_HANDLE);
        return 0;
    };
    let Some((k, a, f)) = w.layered.filter(|_| w.ex_style & WS_EX_LAYERED != 0) else {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    };
    // SAFETY: the caller's optional out-parameters.
    unsafe {
        if !key.is_null() {
            *key = k;
        }
        if !alpha.is_null() {
            *alpha = a;
        }
        if !flags.is_null() {
            *flags = f;
        }
    }
    1
}

const AW_HOR_POSITIVE: u32 = 0x1;
const AW_HOR_NEGATIVE: u32 = 0x2;
const AW_VER_POSITIVE: u32 = 0x4;
const AW_VER_NEGATIVE: u32 = 0x8;
const AW_CENTER: u32 = 0x10;
const AW_HIDE: u32 = 0x1_0000;
const AW_ACTIVATE: u32 = 0x2_0000;
const AW_BLEND: u32 = 0x8_0000;
const SW_HIDE: i32 = 0;
const SW_SHOW: i32 = 5;
const SW_SHOWNA: i32 = 8;

/// AnimateWindow(hwnd, time, flags): shows (activated with AW_ACTIVATE)
/// or hides (AW_HIDE) the window. Fails, as documented, for a window
/// already in that state, a slide or roll with no direction, AW_BLEND on
/// a child window and a window of another thread.
#[unsafe(no_mangle)]
pub extern "system" fn AnimateWindow(hwnd: Hwnd, _time: u32, flags: u32) -> i32 {
    let Some(w) = window(hwnd) else {
        set_last_error(ERROR_INVALID_WINDOW_HANDLE);
        return 0;
    };
    let directions = AW_HOR_POSITIVE | AW_HOR_NEGATIVE | AW_VER_POSITIVE | AW_VER_NEGATIVE;
    let hide = flags & AW_HIDE != 0;
    let visible = crate::window::IsWindowVisible(hwnd) != 0;
    if w.thread != me() {
        set_last_error(ERROR_ACCESS_DENIED);
        return 0;
    }
    if hide != visible
        || (flags & (AW_BLEND | AW_CENTER) == 0 && flags & directions == 0)
        || (flags & AW_BLEND != 0 && w.parent != 0)
    {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    unsupported(b"user32.dll!AnimateWindow(no animation)", u64::from(flags));
    let cmd = if hide {
        SW_HIDE
    } else if flags & AW_ACTIVATE != 0 {
        SW_SHOW
    } else {
        SW_SHOWNA
    };
    ShowWindow(hwnd, cmd);
    1
}

/// DisableProcessWindowsGhosting: nothing to disable (gala does not
/// replace hung windows with ghosts).
#[unsafe(no_mangle)]
pub extern "system" fn DisableProcessWindowsGhosting() {}

/// The shutdown-block reasons (window, text) of this process's windows.
struct Reasons {
    lock: core::sync::atomic::AtomicBool,
    items: core::cell::UnsafeCell<Vec<(Hwnd, Vec<u16>)>>,
}

// SAFETY: `items` is only touched under `lock`.
unsafe impl Sync for Reasons {}

static REASONS: Reasons = Reasons {
    lock: core::sync::atomic::AtomicBool::new(false),
    items: core::cell::UnsafeCell::new(Vec::new()),
};

fn reasons<R>(f: impl FnOnce(&mut Vec<(Hwnd, Vec<u16>)>) -> R) -> R {
    use core::sync::atomic::Ordering;
    while REASONS.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err()
    {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = f(unsafe { &mut *REASONS.items.get() });
    REASONS.lock.store(false, Ordering::Release);
    r
}

/// A window of this thread (ShutdownBlockReason* are for one's own).
fn own(hwnd: Hwnd) -> bool {
    match window(hwnd) {
        Some(w) if w.thread == me() => true,
        Some(_) => {
            set_last_error(ERROR_ACCESS_DENIED);
            false
        }
        None => {
            set_last_error(ERROR_INVALID_WINDOW_HANDLE);
            false
        }
    }
}

/// ShutdownBlockReasonCreate(hwnd, reason): the window's reason (replaced
/// if it had one).
#[unsafe(no_mangle)]
pub extern "system" fn ShutdownBlockReasonCreate(hwnd: Hwnd, reason: *const u16) -> i32 {
    if !own(hwnd) {
        return 0;
    }
    if reason.is_null() {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    while unsafe { *reason.add(n) } != 0 {
        n += 1;
    }
    let mut text = Vec::new();
    if text.try_reserve_exact(n).is_err() {
        set_last_error(8); // ERROR_NOT_ENOUGH_MEMORY
        return 0;
    }
    // SAFETY: the n characters just counted.
    text.extend_from_slice(unsafe { core::slice::from_raw_parts(reason, n) });
    let ok = reasons(|items| {
        // Windows that are gone take their reasons with them.
        items.retain(|e| index_of(e.0).is_some());
        if let Some(e) = items.iter_mut().find(|e| e.0 == hwnd) {
            e.1 = text;
            return true;
        }
        if items.try_reserve(1).is_err() {
            return false;
        }
        items.push((hwnd, text));
        true
    });
    if !ok {
        set_last_error(8);
    }
    i32::from(ok)
}

/// ShutdownBlockReasonQuery(hwnd, buffer, *chars): the reason; with no
/// buffer *chars is set to its length with the NUL; FALSE when the
/// window has none.
#[unsafe(no_mangle)]
pub extern "system" fn ShutdownBlockReasonQuery(
    hwnd: Hwnd,
    buffer: *mut u16,
    chars: *mut u32,
) -> i32 {
    if !own(hwnd) {
        return 0;
    }
    if chars.is_null() {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    let r = reasons(|items| {
        let e = items.iter().find(|e| e.0 == hwnd)?;
        let need = e.1.len() as u32 + 1;
        // SAFETY: the caller's count.
        let room = unsafe { *chars };
        if buffer.is_null() {
            return Some(Ok(need));
        }
        if room < need {
            return Some(Err(need));
        }
        // SAFETY: the caller's buffer of `room` characters.
        unsafe {
            core::ptr::copy_nonoverlapping(e.1.as_ptr(), buffer, e.1.len());
            *buffer.add(e.1.len()) = 0;
        }
        Some(Ok(need))
    });
    match r {
        None => {
            set_last_error(2); // ERROR_FILE_NOT_FOUND: no reason
            0
        }
        Some(Ok(need)) => {
            // SAFETY: the caller's count.
            unsafe { *chars = need };
            1
        }
        Some(Err(need)) => {
            // SAFETY: the caller's count.
            unsafe { *chars = need };
            set_last_error(ERROR_INSUFFICIENT_BUFFER);
            0
        }
    }
}

/// ShutdownBlockReasonDestroy(hwnd): the window's reason goes (TRUE even
/// when it had none, as the documentation describes no failure for it).
#[unsafe(no_mangle)]
pub extern "system" fn ShutdownBlockReasonDestroy(hwnd: Hwnd) -> i32 {
    if !own(hwnd) {
        return 0;
    }
    reasons(|items| items.retain(|e| e.0 != hwnd));
    1
}

/// GetLastActivePopup(hwnd): the popup owned by `hwnd` that was activated
/// last, if it is still there and shown; otherwise `hwnd` itself (also
/// for a window that is owned, as documented).
#[unsafe(no_mangle)]
pub extern "system" fn GetLastActivePopup(hwnd: Hwnd) -> Hwnd {
    let Some(w) = window(hwnd) else {
        set_last_error(ERROR_INVALID_WINDOW_HANDLE);
        return 0;
    };
    if w.owner != 0 || w.parent != 0 || w.last_popup == 0 {
        return hwnd;
    }
    match window(w.last_popup) {
        Some(p) if p.owner == hwnd && crate::window::IsWindowVisible(w.last_popup) != 0 => {
            w.last_popup
        }
        _ => hwnd,
    }
}

/// ShowOwnedPopups(hwnd, show): FALSE hides the shown popups `hwnd` owns,
/// TRUE shows again the ones it hid (without activating them); TRUE.
#[unsafe(no_mangle)]
pub extern "system" fn ShowOwnedPopups(hwnd: Hwnd, show: i32) -> i32 {
    if window(hwnd).is_none() {
        set_last_error(ERROR_INVALID_WINDOW_HANDLE);
        return 0;
    }
    let mut owned = [0 as Hwnd; 64];
    let mut n = 0;
    with(|s| {
        for (i, w) in s.windows.iter_mut().enumerate() {
            if !(w.used && w.owner == hwnd && n < owned.len()) {
                continue;
            }
            let pick = if show != 0 { w.hidden_with_owner } else { w.visible };
            if pick {
                w.hidden_with_owner = show == 0;
                owned[n] = crate::window::hwnd_of(i);
                n += 1;
            }
        }
    });
    if n == owned.len() {
        unsupported(b"user32.dll!ShowOwnedPopups(over 64 popups)", n as u64);
    }
    for &p in &owned[..n] {
        ShowWindow(p, if show != 0 { SW_SHOWNA } else { SW_HIDE });
    }
    1
}
