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

// ---- Cursor, keyboard state, display queries, pointer input ---------------

/// ClipCursor(lpRect): accepted; gala does not confine the cursor yet.
#[unsafe(no_mangle)]
pub extern "system" fn ClipCursor(_rect: *const i32) -> i32 {
    1
}

/// GetCursorInfo(pci): CURSORINFO { cbSize (24), flags, hCursor,
/// ptScreenPos }: showing, the arrow (gala's cursor is not a handle: 0),
/// at the cursor's position.
#[unsafe(no_mangle)]
pub extern "system" fn GetCursorInfo(info: *mut u8) -> i32 {
    // SAFETY: the caller's CURSORINFO.
    unsafe {
        if info.is_null() || info.cast::<u32>().read_unaligned() != 24 {
            set_error(87);
            return 0;
        }
        info.add(4).cast::<u32>().write_unaligned(1); // CURSOR_SHOWING
        info.add(8).cast::<usize>().write_unaligned(0);
        let mut point = [0i32; 2];
        super::window::GetCursorPos(&mut point);
        info.add(16).cast::<[i32; 2]>().write_unaligned(point);
    }
    1
}

/// SetKeyboardState(lpKeyState): accepted (the next GetKeyboardState is
/// the real keyboard's; gala keeps no per-thread copy).
#[unsafe(no_mangle)]
pub extern "system" fn SetKeyboardState(state: *const u8) -> i32 {
    if state.is_null() {
        set_error(87);
        return 0;
    }
    1
}

/// EnumDisplayDevicesW(lpDevice, iDevNum, lpDisplayDevice, dwFlags): one
/// adapter, `\\.\DISPLAY1`, attached to the desktop and primary.
#[unsafe(no_mangle)]
pub extern "system" fn EnumDisplayDevicesW(
    device: *const u16,
    index: u32,
    out: *mut u8,
    _flags: u32,
) -> i32 {
    // DISPLAY_DEVICEW: cb, DeviceName[32], DeviceString[128], StateFlags,
    // DeviceID[128], DeviceKey[128] (840 bytes).
    // SAFETY: the caller's structure (its cb says how much is there).
    unsafe {
        if out.is_null() || out.cast::<u32>().read_unaligned() < 4 + 64 + 256 + 4 {
            set_error(87);
            return 0;
        }
        if !device.is_null() || index != 0 {
            return 0; // no monitors listed under the adapter, one adapter
        }
        let size = out.cast::<u32>().read_unaligned() as usize;
        core::ptr::write_bytes(out.add(4), 0, size.min(840) - 4);
        let put = |at: usize, text: &[u8]| {
            for (i, &c) in text.iter().enumerate() {
                out.add(at + 2 * i).cast::<u16>().write_unaligned(u16::from(c));
            }
        };
        put(4, b"\\\\.\\DISPLAY1");
        put(68, b"gala display");
        out.add(324).cast::<u32>().write_unaligned(0x1 | 0x4); // ATTACHED | PRIMARY
    }
    1
}

/// GetDisplayConfigBufferSizes / QueryDisplayConfig /
/// DisplayConfigGetDeviceInfo: the CCD display configuration is not
/// supported (ERROR_NOT_SUPPORTED, their documented "not available").
#[unsafe(no_mangle)]
pub extern "system" fn GetDisplayConfigBufferSizes(
    _flags: u32,
    _paths: *mut u32,
    _modes: *mut u32,
) -> i32 {
    50
}

#[unsafe(no_mangle)]
pub extern "system" fn QueryDisplayConfig(
    _flags: u32,
    _paths: *mut u32,
    _path_info: *mut u8,
    _modes: *mut u32,
    _mode_info: *mut u8,
    _topology: *mut u32,
) -> i32 {
    50
}

#[unsafe(no_mangle)]
pub extern "system" fn DisplayConfigGetDeviceInfo(_packet: *mut u8) -> i32 {
    50
}

/// GetPointerType / GetPointerDevice / GetPointerPenInfo: there are no
/// pointers on gala, so every pointer id is unknown.
#[unsafe(no_mangle)]
pub extern "system" fn GetPointerType(_id: u32, _kind: *mut u32) -> i32 {
    set_error(87);
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn GetPointerDevice(_device: usize, _info: *mut u8) -> i32 {
    set_error(87);
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn GetPointerPenInfo(_id: u32, _info: *mut u8) -> i32 {
    set_error(87);
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn CloseTouchInputHandle(_handle: usize) -> i32 {
    set_error(6);
    0
}

/// GetRawInputData / GetRawInputDeviceInfoW: no raw input ever arrives,
/// so no handle is valid ((UINT)-1, ERROR_INVALID_HANDLE).
#[unsafe(no_mangle)]
pub extern "system" fn GetRawInputData(
    _input: usize,
    _command: u32,
    _data: *mut u8,
    _size: *mut u32,
    _header: u32,
) -> u32 {
    set_error(6);
    u32::MAX
}

#[unsafe(no_mangle)]
pub extern "system" fn GetRawInputDeviceInfoW(
    _device: usize,
    _command: u32,
    _data: *mut u8,
    _size: *mut u32,
) -> u32 {
    set_error(6);
    u32::MAX
}

#[unsafe(no_mangle)]
pub extern "system" fn DefRawInputProc(_input: *const usize, _count: i32, _header: u32) -> isize {
    0
}

/// SetWindowDisplayAffinity: accepted (gala's screen capture is not
/// restricted, so WDA_NONE is what holds; other values are kept by nobody).
#[unsafe(no_mangle)]
pub extern "system" fn SetWindowDisplayAffinity(hwnd: usize, _affinity: u32) -> i32 {
    if hwnd == 0 {
        set_error(1400);
        return 0;
    }
    1
}

/// WindowFromDC: gala's DCs are not tied to windows it can name (NULL).
#[unsafe(no_mangle)]
pub extern "system" fn WindowFromDC(_dc: usize) -> usize {
    0
}

// ---- Window properties (SetProp/GetProp/RemoveProp) -----------------------
//
// A process-wide table of (window, name, value); a name is an atom (high
// word 0) or a string, kept as a case-insensitive hash (FNV-1a of the
// upper-cased UTF-16). Up to PROPS properties at once.

const PROPS: usize = 512;

#[derive(Clone, Copy)]
struct Prop {
    hwnd: usize,
    name: u64,
    value: usize,
}

struct PropTable {
    lock: core::sync::atomic::AtomicBool,
    props: core::cell::UnsafeCell<[Prop; PROPS]>,
}
// SAFETY: `props` is only used under `lock`.
unsafe impl Sync for PropTable {}

static PROP_TABLE: PropTable = PropTable {
    lock: core::sync::atomic::AtomicBool::new(false),
    props: core::cell::UnsafeCell::new([Prop { hwnd: 0, name: 0, value: 0 }; PROPS]),
};

fn prop_name(name: *const u16) -> Option<u64> {
    if name.is_null() {
        return None;
    }
    if (name as usize) >> 16 == 0 {
        return Some(name as u64); // an atom
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    // SAFETY: the caller's NUL-terminated string.
    unsafe {
        while *name.add(i) != 0 {
            let c = *name.add(i);
            let c = if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c };
            h = (h ^ u64::from(c)).wrapping_mul(0x0100_0000_01b3);
            i += 1;
        }
    }
    Some(h | 1 << 63)
}

fn with_props<R>(f: impl FnOnce(&mut [Prop; PROPS]) -> R) -> R {
    use core::sync::atomic::Ordering;
    while PROP_TABLE
        .lock
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = f(unsafe { &mut *PROP_TABLE.props.get() });
    PROP_TABLE.lock.store(false, Ordering::Release);
    r
}

/// SetPropW(hWnd, lpString, hData).
#[unsafe(no_mangle)]
pub extern "system" fn SetPropW(hwnd: usize, name: *const u16, value: usize) -> i32 {
    let Some(key) = prop_name(name) else {
        set_error(87);
        return 0;
    };
    if hwnd == 0 {
        set_error(1400);
        return 0;
    }
    let ok = with_props(|props| {
        if let Some(p) = props.iter_mut().find(|p| p.hwnd == hwnd && p.name == key) {
            p.value = value;
            return true;
        }
        match props.iter_mut().find(|p| p.hwnd == 0) {
            Some(p) => {
                *p = Prop { hwnd, name: key, value };
                true
            }
            None => false,
        }
    });
    if !ok {
        set_error(8); // ERROR_NOT_ENOUGH_MEMORY
    }
    i32::from(ok)
}

/// GetPropW(hWnd, lpString): the value, or NULL.
#[unsafe(no_mangle)]
pub extern "system" fn GetPropW(hwnd: usize, name: *const u16) -> usize {
    let Some(key) = prop_name(name) else { return 0 };
    with_props(|props| {
        props.iter().find(|p| p.hwnd == hwnd && p.name == key).map_or(0, |p| p.value)
    })
}

/// RemovePropW(hWnd, lpString): the value, the property gone.
#[unsafe(no_mangle)]
pub extern "system" fn RemovePropW(hwnd: usize, name: *const u16) -> usize {
    let Some(key) = prop_name(name) else { return 0 };
    with_props(|props| match props.iter_mut().find(|p| p.hwnd == hwnd && p.name == key) {
        Some(p) => {
            let v = p.value;
            *p = Prop { hwnd: 0, name: 0, value: 0 };
            v
        }
        None => 0,
    })
}
