//! Small session-wide queries Chromium's browser makes at start-up: the
//! keyboard layouts (US English only, as GetKeyboardLayout says), the last input
//! time, a message's extra information and GUI resource counts.

use core::sync::atomic::Ordering;

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

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
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
/// CURSORINFO (24 bytes on x64, 20 on x86).
#[repr(C)]
pub struct CursorInfo {
    size: u32,
    flags: u32,
    cursor: usize,
    point: [i32; 2],
}

#[unsafe(no_mangle)]
pub extern "system" fn GetCursorInfo(info: *mut CursorInfo) -> i32 {
    // SAFETY: the caller's CURSORINFO.
    unsafe {
        if info.is_null() || info.cast::<u32>().read_unaligned() != size_of::<CursorInfo>() as u32 {
            set_error(87);
            return 0;
        }
        let mut point = [0i32; 2];
        super::window::GetCursorPos(&mut point);
        // CURSOR_SHOWING.
        info.write_unaligned(CursorInfo {
            size: size_of::<CursorInfo>() as u32,
            flags: 1,
            cursor: 0,
            point,
        });
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

/// WindowFromDC: the window of a DC GetDC, GetWindowDC or BeginPaint
/// handed out and not yet released; NULL for others (memory DCs, the
/// screen's).
#[unsafe(no_mangle)]
pub extern "system" fn WindowFromDC(dc: usize) -> usize {
    super::window::dc_window(dc)
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

// ---- Caret, beeps, cursor count, keyboard state, display modes ------------

/// GetCaretBlinkTime: Windows' default, 530 ms.
#[unsafe(no_mangle)]
pub extern "system" fn GetCaretBlinkTime() -> u32 {
    530
}

/// MessageBeep(uType): gala has no sound; the beep is silent (TRUE).
#[unsafe(no_mangle)]
pub extern "system" fn MessageBeep(_kind: u32) -> i32 {
    1
}

static CURSOR_COUNT: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(0);

/// ShowCursor(bShow): the display count after the change (the cursor
/// shows while it is 0 or more; gala's cursor is drawn regardless).
#[unsafe(no_mangle)]
pub extern "system" fn ShowCursor(show: i32) -> i32 {
    let delta = if show != 0 { 1 } else { -1 };
    CURSOR_COUNT.fetch_add(delta, Ordering::Relaxed) + delta
}

/// GetKeyboardState(lpKeyState): 256 key states, each from
/// GetAsyncKeyState (down: 0x80).
#[unsafe(no_mangle)]
pub extern "system" fn GetKeyboardState(state: *mut u8) -> i32 {
    if state.is_null() {
        set_error(87);
        return 0;
    }
    for vk in 0..256 {
        let down = super::window::GetAsyncKeyState(vk) as u16 & 0x8000 != 0;
        // SAFETY: the caller's 256-byte array.
        unsafe { *state.add(vk as usize) = if down { 0x80 } else { 0 } };
    }
    1
}

/// EnumDisplaySettingsW(lpszDeviceName, iModeNum, lpDevMode): the
/// screen's one mode (ENUM_CURRENT_SETTINGS, ENUM_REGISTRY_SETTINGS or
/// mode 0): its size, 32 bits per pixel, 60 Hz, at 0,0.
#[unsafe(no_mangle)]
pub extern "system" fn EnumDisplaySettingsW(
    _device: *const u16,
    mode: u32,
    devmode: *mut u8,
) -> i32 {
    if devmode.is_null() || !matches!(mode, 0 | 0xffff_ffff | 0xffff_fffe) {
        return 0;
    }
    let (w, h) = (super::window::GetSystemMetrics(0), super::window::GetSystemMetrics(1));
    // SAFETY: the caller's DEVMODEW (dmSize says how much; 220 bytes).
    unsafe {
        let size = usize::from(devmode.add(68).cast::<u16>().read_unaligned()).max(188);
        core::ptr::write_bytes(devmode.add(72), 0, size.min(220) - 72);
        devmode
            .add(72)
            .cast::<u32>()
            .write_unaligned(0x20 | 0x4_0000 | 0x8_0000 | 0x10_0000 | 0x40_0000);
        devmode.add(168).cast::<u32>().write_unaligned(32);
        devmode.add(172).cast::<u32>().write_unaligned(w as u32);
        devmode.add(176).cast::<u32>().write_unaligned(h as u32);
        devmode.add(184).cast::<u32>().write_unaligned(60);
    }
    1
}

/// EnumDisplaySettingsA: EnumDisplaySettingsW's mode in DEVMODEA's
/// layout (32-byte names: dmSize at 36, dmFields at 40, dmBitsPerPel at
/// 104, the size at 108 and 112, the frequency at 120).
#[unsafe(no_mangle)]
pub extern "system" fn EnumDisplaySettingsA(
    _device: *const u8,
    mode: u32,
    devmode: *mut u8,
) -> i32 {
    if devmode.is_null() {
        return 0;
    }
    let mut wide = [0u8; 220];
    wide[68..70].copy_from_slice(&220u16.to_le_bytes());
    if EnumDisplaySettingsW(core::ptr::null(), mode, wide.as_mut_ptr()) == 0 {
        return 0;
    }
    let field =
        |at: usize| u32::from_le_bytes([wide[at], wide[at + 1], wide[at + 2], wide[at + 3]]);
    // SAFETY: the caller's DEVMODEA (dmSize says how much; 156 bytes).
    unsafe {
        let size = usize::from(devmode.add(36).cast::<u16>().read_unaligned()).max(124);
        core::ptr::write_bytes(devmode.add(40), 0, size.min(156) - 40);
        devmode.add(40).cast::<u32>().write_unaligned(field(72));
        for (a, w) in [(104, 168), (108, 172), (112, 176), (120, 184)] {
            devmode.add(a).cast::<u32>().write_unaligned(field(w));
        }
    }
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn EnumDisplaySettingsExW(
    device: *const u16,
    mode: u32,
    devmode: *mut u8,
    _flags: u32,
) -> i32 {
    EnumDisplaySettingsW(device, mode, devmode)
}

// ---- ANSI window classes and windows -----------------------------------------

/// A class or window name given in ANSI (UTF-8) as UTF-16 in `buf`; atoms
/// (values under 0x10000) and NULL pass through.
fn wide_name(name: *const u8, buf: &mut [u16; 260]) -> *const u16 {
    if (name as usize) < 0x1_0000 {
        return name.cast();
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    unsafe {
        while *name.add(n) != 0 && n < buf.len() - 1 {
            buf[n] = u16::from(*name.add(n));
            n += 1;
        }
    }
    buf[n] = 0;
    buf.as_ptr()
}

/// CreateWindowExA: CreateWindowExW with the class and window names
/// converted (ASCII; gala's ANSI code page is UTF-8, and names beyond
/// ASCII are not used by the programs that call this).
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn CreateWindowExA(
    ex_style: u32,
    class: *const u8,
    name: *const u8,
    style: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    parent: usize,
    menu: *mut core::ffi::c_void,
    instance: *mut core::ffi::c_void,
    param: *mut core::ffi::c_void,
) -> usize {
    let (mut c, mut t) = ([0u16; 260], [0u16; 260]);
    let class = wide_name(class, &mut c);
    let title = wide_name(name, &mut t);
    super::window::CreateWindowExW(
        ex_style, class, title, style, x, y, w, h, parent, menu, instance, param,
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn UnregisterClassA(name: *const u8, instance: usize) -> i32 {
    let mut c = [0u16; 260];
    super::window::UnregisterClassW(wide_name(name, &mut c), instance)
}

// ---- Layered windows, regions, printing, input injection -------------------

/// UpdateLayeredWindow: not supported yet (reported; FALSE).
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn UpdateLayeredWindow(
    _hwnd: usize,
    _dst: usize,
    _pos: *const i32,
    _size: *const i32,
    _src: usize,
    _src_pos: *const i32,
    _key: u32,
    _blend: *const u8,
    _flags: u32,
) -> i32 {
    super::window::unsupported(b"user32.dll!UpdateLayeredWindow", 0);
    set_error(50);
    0
}

/// GetWindowRgn / GetWindowRgnBox: windows have no region on gala (ERROR,
/// 0).
#[unsafe(no_mangle)]
pub extern "system" fn GetWindowRgn(_hwnd: usize, _rgn: usize) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn GetWindowRgnBox(_hwnd: usize, _rect: *mut i32) -> i32 {
    0
}

/// PrintWindow: not supported yet (reported; FALSE).
#[unsafe(no_mangle)]
pub extern "system" fn PrintWindow(_hwnd: usize, _dc: usize, _flags: u32) -> i32 {
    super::window::unsupported(b"user32.dll!PrintWindow", 0);
    0
}

/// SendInput: synthesized input is not supported yet (reported; 0 events
/// inserted, ERROR_NOT_SUPPORTED).
#[unsafe(no_mangle)]
pub extern "system" fn SendInput(count: u32, _inputs: *const u8, _size: i32) -> u32 {
    super::window::unsupported(b"user32.dll!SendInput", u64::from(count));
    set_error(50);
    0
}

/// FrameRect(hDC, lprc, hbr): a one-pixel border with the brush (four
/// FillRect bands).
#[unsafe(no_mangle)]
pub extern "system" fn FrameRect(
    dc: *mut core::ffi::c_void,
    rect: *const [i32; 4],
    brush: *mut core::ffi::c_void,
) -> i32 {
    if rect.is_null() {
        return 0;
    }
    // SAFETY: the caller's RECT.
    let [l, t, r, b] = unsafe { *rect };
    if r <= l || b <= t {
        return 1;
    }
    for band in [[l, t, r, t + 1], [l, b - 1, r, b], [l, t, l + 1, b], [r - 1, t, r, b]] {
        super::window::FillRect(dc, &band, brush);
    }
    1
}

/// InvertRect(hDC, lprc): PatBlt with DSTINVERT.
#[unsafe(no_mangle)]
pub extern "system" fn InvertRect(dc: *mut core::ffi::c_void, rect: *const [i32; 4]) -> i32 {
    if rect.is_null() {
        return 0;
    }
    // SAFETY: the caller's RECT.
    let [l, t, r, b] = unsafe { *rect };
    // gdi32 is looked up when needed (user32 does not import it: programs
    // without gdi32 still load user32).
    static PAT_BLT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    let f = super::icon::gdi_proc(&PAT_BLT, b"PatBlt ");
    if f == 0 {
        return 0;
    }
    type PatBlt = extern "system" fn(*mut core::ffi::c_void, i32, i32, i32, i32, u32) -> i32;
    // SAFETY: gdi32's PatBlt, with its documented signature.
    let pat_blt: PatBlt = unsafe { core::mem::transmute(f) };
    pat_blt(dc, l, t, r - l, b - t, 0x0055_0009)
}

// ---- What chrome.dll imports that has no use on gala yet ------------------
//
// chrome.dll loads every user32 function it delay-imports at once
// (GetProcAddress, which does not return gala_stubs' stubs), so each must
// be a real export: these fail the documented way (or draw nothing) and
// report themselves to the ledger.

/// DdeInitializeW: DDE is not supported (DMLERR_SYS_ERROR; reported).
#[unsafe(no_mangle)]
pub extern "system" fn DdeInitializeW(
    instance: *mut u32,
    _callback: usize,
    _command: u32,
    _reserved: u32,
) -> u32 {
    super::window::unsupported(b"user32.dll!DdeInitializeW", 0);
    if !instance.is_null() {
        // SAFETY: the caller's output.
        unsafe { *instance = 0 };
    }
    0x400f // DMLERR_SYS_ERROR
}

#[unsafe(no_mangle)]
pub extern "system" fn DdeUninitialize(_instance: u32) -> i32 {
    0
}

/// DdeGetLastError: DMLERR_DLL_NOT_INITIALIZED (no instance can exist).
#[unsafe(no_mangle)]
pub extern "system" fn DdeGetLastError(_instance: u32) -> u32 {
    0x4003
}

#[unsafe(no_mangle)]
pub extern "system" fn DdeConnect(
    _instance: u32,
    _service: usize,
    _topic: usize,
    _context: *const u8,
) -> usize {
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn DdeDisconnect(_conversation: usize) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn DdeCreateStringHandleW(
    _instance: u32,
    _text: *const u16,
    _code_page: i32,
) -> usize {
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn DdeFreeStringHandle(_instance: u32, _handle: usize) -> i32 {
    0
}

#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn DdeClientTransaction(
    _data: *const u8,
    _length: u32,
    _conversation: usize,
    _item: usize,
    _format: u32,
    _kind: u32,
    _timeout: u32,
    _result: *mut u32,
) -> usize {
    0
}

/// DrawEdge / DrawFocusRect / DrawFrameControl: 3-D edges, focus
/// rectangles and frame controls are not drawn yet (reported; TRUE, as
/// the drawing itself has no result a caller reads).
#[unsafe(no_mangle)]
pub extern "system" fn DrawEdge(_dc: usize, _rect: *mut i32, _edge: u32, _flags: u32) -> i32 {
    super::window::unsupported(b"user32.dll!DrawEdge", 0);
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn DrawFocusRect(_dc: usize, _rect: *const i32) -> i32 {
    super::window::unsupported(b"user32.dll!DrawFocusRect", 0);
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn DrawFrameControl(
    _dc: usize,
    _rect: *mut i32,
    _kind: u32,
    _state: u32,
) -> i32 {
    super::window::unsupported(b"user32.dll!DrawFrameControl", 0);
    1
}

/// GetClassInfoW: GetClassInfoExW without the small icon (WNDCLASSW is
/// WNDCLASSEXW without cbSize and hIconSm).
#[unsafe(no_mangle)]
pub extern "system" fn GetClassInfoW(
    instance: *mut core::ffi::c_void,
    name: *const u16,
    out: *mut super::window::WndClassW,
) -> i32 {
    if out.is_null() {
        set_error(87);
        return 0;
    }
    let mut ex = super::window::WndClassExW::EMPTY;
    let atom = super::window::GetClassInfoExW(instance, name, &mut ex);
    if atom != 0 {
        // SAFETY: the caller's WNDCLASSW.
        unsafe { out.write_unaligned(ex.without_size()) };
    }
    atom
}

/// GetWindowDC: the window's DC (gala's DCs cover the whole window).
#[unsafe(no_mangle)]
pub extern "system" fn GetWindowDC(hwnd: usize) -> *mut core::ffi::c_void {
    super::window::GetDC(hwnd)
}

/// GetWindowPlacement(hWnd, lpwndpl): WINDOWPLACEMENT { length, flags,
/// showCmd, ptMinPosition, ptMaxPosition, rcNormalPosition }: shown
/// normally (or minimized/maximized), the window's rectangle as normal.
#[unsafe(no_mangle)]
pub extern "system" fn GetWindowPlacement(hwnd: usize, placement: *mut u8) -> i32 {
    if placement.is_null() {
        set_error(87);
        return 0;
    }
    let mut rect = [0i32; 4];
    if super::tree::GetWindowRect(hwnd, &mut rect) == 0 {
        return 0;
    }
    let show = if super::tree::IsIconic(hwnd) != 0 {
        2
    } else if super::tree::IsZoomed(hwnd) != 0 {
        3
    } else {
        1
    };
    // SAFETY: the caller's 44-byte WINDOWPLACEMENT.
    unsafe {
        placement.add(4).cast::<u32>().write_unaligned(0);
        placement.add(8).cast::<u32>().write_unaligned(show);
        core::ptr::write_bytes(placement.add(12), 0xff, 16); // -1, -1 positions
        placement.add(28).cast::<[i32; 4]>().write_unaligned(rect);
    }
    1
}

/// SetWindowPlacement: the normal rectangle (SetWindowPos) and showCmd.
#[unsafe(no_mangle)]
pub extern "system" fn SetWindowPlacement(hwnd: usize, placement: *const u8) -> i32 {
    if placement.is_null() {
        set_error(87);
        return 0;
    }
    // SAFETY: the caller's WINDOWPLACEMENT.
    let (show, [l, t, r, b]) = unsafe {
        (
            placement.add(8).cast::<u32>().read_unaligned(),
            placement.add(28).cast::<[i32; 4]>().read_unaligned(),
        )
    };
    super::tree::SetWindowPos(hwnd, 0, l, t, r - l, b - t, 0x14); // NOZORDER | NOACTIVATE
    super::window::ShowWindow(hwnd, show as i32);
    1
}

/// RegisterHotKey / UnregisterHotKey: global hot keys are not delivered
/// yet (reported; the registration is accepted).
#[unsafe(no_mangle)]
pub extern "system" fn RegisterHotKey(_hwnd: usize, id: i32, _modifiers: u32, vk: u32) -> i32 {
    super::window::unsupported(b"user32.dll!RegisterHotKey", (id as u64) << 16 | u64::from(vk));
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn UnregisterHotKey(_hwnd: usize, _id: i32) -> i32 {
    1
}

/// SetCursorPos: moving the cursor from a program is not supported yet
/// (reported; FALSE, ERROR_ACCESS_DENIED as for a desktop that refuses).
#[unsafe(no_mangle)]
pub extern "system" fn SetCursorPos(x: i32, y: i32) -> i32 {
    super::window::unsupported(
        b"user32.dll!SetCursorPos",
        (x as u32 as u64) << 32 | u64::from(y as u32),
    );
    set_error(5);
    0
}

/// SetWindowRgn: window regions are not kept (reported; the call
/// succeeds, the window stays rectangular).
#[unsafe(no_mangle)]
pub extern "system" fn SetWindowRgn(_hwnd: usize, _rgn: usize, _redraw: i32) -> i32 {
    super::window::unsupported(b"user32.dll!SetWindowRgn", 0);
    1
}
