//! Small session-wide queries Chromium's browser makes at start-up: the
//! clipboard's listeners and sequence number (gala's clipboard never
//! changes yet, so a listener is never told anything), the keyboard
//! layouts (US English only, as GetKeyboardLayout says), the last input
//! time, a message's extra information and GUI resource counts.

use core::sync::atomic::{AtomicU32, Ordering};

static CLIPBOARD_LISTENERS: AtomicU32 = AtomicU32::new(0);

/// AddClipboardFormatListener(hwnd): WM_CLIPBOARDUPDATE would be posted
/// on changes; there are none.
#[unsafe(no_mangle)]
pub extern "system" fn AddClipboardFormatListener(hwnd: usize) -> i32 {
    if hwnd == 0 {
        set_error(1400); // ERROR_INVALID_WINDOW_HANDLE
        return 0;
    }
    CLIPBOARD_LISTENERS.fetch_add(1, Ordering::Relaxed);
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn RemoveClipboardFormatListener(hwnd: usize) -> i32 {
    if hwnd == 0 {
        set_error(1400);
        return 0;
    }
    CLIPBOARD_LISTENERS.fetch_sub(1, Ordering::Relaxed);
    1
}

/// GetClipboardSequenceNumber: 0 (the clipboard has never changed).
#[unsafe(no_mangle)]
pub extern "system" fn GetClipboardSequenceNumber() -> u32 {
    0
}

/// GetKeyboardLayoutList(nBuff, lpList): one layout, US English.
#[unsafe(no_mangle)]
pub extern "system" fn GetKeyboardLayoutList(count: i32, list: *mut usize) -> i32 {
    if count > 0 && !list.is_null() {
        // SAFETY: the caller's array of `count` HKLs.
        unsafe { *list = super::keyboard::GetKeyboardLayout(0) };
    }
    1
}

/// GetKeyboardLayoutNameW(pwszKLID): "00000409" (KL_NAMELENGTH 9).
#[unsafe(no_mangle)]
pub extern "system" fn GetKeyboardLayoutNameW(name: *mut u16) -> i32 {
    if name.is_null() {
        set_error(87);
        return 0;
    }
    for (i, &c) in b"00000409\0".iter().enumerate() {
        // SAFETY: the caller's buffer of KL_NAMELENGTH characters.
        unsafe { *name.add(i) = u16::from(c) };
    }
    1
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetTickCount() -> u32;
}

/// GetLastInputInfo(plii): LASTINPUTINFO { cbSize (8), dwTime }; gala does
/// not track input times yet, so the time is now (a user just active).
#[unsafe(no_mangle)]
pub extern "system" fn GetLastInputInfo(info: *mut u32) -> i32 {
    // SAFETY: the caller's LASTINPUTINFO.
    unsafe {
        if info.is_null() || *info != 8 {
            set_error(87);
            return 0;
        }
        *info.add(1) = GetTickCount();
    }
    1
}

/// GetMessageExtraInfo: 0 (gala's input carries no extra information).
#[unsafe(no_mangle)]
pub extern "system" fn GetMessageExtraInfo() -> isize {
    0
}

/// GetGuiResources(hProcess, uiFlags): 0 GDI and USER objects counted
/// (gala does not count them per process yet).
#[unsafe(no_mangle)]
pub extern "system" fn GetGuiResources(_process: usize, _flags: u32) -> u32 {
    0
}

fn set_error(code: u32) {
    // SAFETY: plain call.
    unsafe { super::SetLastError(code) };
}

// ---- Devices and foreground rights ----------------------------------------
//
// gala has no pointer (pen, touch) devices, no raw input devices and no
// device-interface notifications yet: the lists are empty, and a
// registration is kept by nobody (nothing is ever reported for it).

/// AllowSetForegroundWindow(dwProcessId): granted (gala does not restrict
/// SetForegroundWindow).
#[unsafe(no_mangle)]
pub extern "system" fn AllowSetForegroundWindow(_process: u32) -> i32 {
    1
}

/// GetPointerDevices(*deviceCount, pointerDevices): none.
#[unsafe(no_mangle)]
pub extern "system" fn GetPointerDevices(count: *mut u32, _devices: *mut u8) -> i32 {
    if count.is_null() {
        set_error(87);
        return 0;
    }
    // SAFETY: the caller's output.
    unsafe { *count = 0 };
    1
}

/// RegisterPointerDeviceNotifications(window, notifyRange): accepted.
#[unsafe(no_mangle)]
pub extern "system" fn RegisterPointerDeviceNotifications(hwnd: usize, _range: i32) -> i32 {
    if hwnd == 0 {
        set_error(1400);
        return 0;
    }
    1
}

/// GetRawInputDeviceList(pRawInputDeviceList, puiNumDevices, cbSize): no
/// devices (0).
#[unsafe(no_mangle)]
pub extern "system" fn GetRawInputDeviceList(_list: *mut u8, count: *mut u32, size: u32) -> u32 {
    if count.is_null() || size != 16 {
        set_error(87);
        return u32::MAX;
    }
    // SAFETY: the caller's output.
    unsafe { *count = 0 };
    0
}

/// RegisterRawInputDevices(pRawInputDevices, uiNumDevices, cbSize):
/// accepted; no WM_INPUT is ever sent (no raw input devices).
#[unsafe(no_mangle)]
pub extern "system" fn RegisterRawInputDevices(devices: *const u8, count: u32, size: u32) -> i32 {
    if (devices.is_null() && count != 0) || size != 16 {
        set_error(87);
        return 0;
    }
    1
}

static NEXT_DEVICE_NOTIFY: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0x4445_0000);

/// RegisterDeviceNotificationW(hRecipient, NotificationFilter, Flags): a
/// distinct handle; no device arrives or leaves on gala.
#[unsafe(no_mangle)]
pub extern "system" fn RegisterDeviceNotificationW(
    recipient: usize,
    filter: *const u8,
    _flags: u32,
) -> usize {
    if recipient == 0 || filter.is_null() {
        set_error(87);
        return 0;
    }
    NEXT_DEVICE_NOTIFY.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "system" fn UnregisterDeviceNotification(handle: usize) -> i32 {
    if handle == 0 {
        set_error(6); // ERROR_INVALID_HANDLE
        return 0;
    }
    1
}

/// GetWindowDisplayAffinity(hWnd, *pdwAffinity): WDA_NONE (capture is not
/// restricted on gala).
#[unsafe(no_mangle)]
pub extern "system" fn GetWindowDisplayAffinity(hwnd: usize, affinity: *mut u32) -> i32 {
    if hwnd == 0 || affinity.is_null() {
        set_error(87);
        return 0;
    }
    // SAFETY: the caller's output.
    unsafe { *affinity = 0 };
    1
}

/// IsWindowArranged(hwnd): FALSE (gala's snap layouts do not mark windows).
#[unsafe(no_mangle)]
pub extern "system" fn IsWindowArranged(_hwnd: usize) -> i32 {
    0
}

/// GetShellWindow: NULL (no shell desktop window owned by a shell process).
#[unsafe(no_mangle)]
pub extern "system" fn GetShellWindow() -> usize {
    0
}
