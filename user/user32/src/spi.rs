//! SystemParametersInfoW: gala's settings, with Windows' defaults where
//! gala has no choice of its own: the non-client metrics (a 1-pixel
//! border, the desktop's 30-pixel caption, 17-pixel scroll bars, the GUI
//! font: 12-pixel Segoe UI, which gdi32 draws in Noto Sans Mono), the work
//! area above the taskbar, no high contrast, three lines per wheel notch,
//! keyboard cues shown only after Alt, flat menus, UI effects off. Unknown
//! actions go to the ledger and fail.
//!
//! The wallpaper is the user's `Control Panel\Desktop` "Wallpaper" value,
//! where Windows keeps it: SPI_GETDESKWALLPAPER reads it (the key of the
//! process token's user), SPI_SETDESKWALLPAPER asks the desktop for it
//! (GalaDesktopAction ACTION_WALLPAPER), which shows it, saves the value
//! (whatever the SPIF_ flags say) and tells the shell's window
//! (WM_SETTINGCHANGE); gala shows `.qoi` pictures only.

use crate::window::{CAPTION, set_last_error};

/// LOGFONTW for the GUI font.
fn gui_font(out: *mut u8) {
    let mut lf = [0u8; 92];
    lf[0..4].copy_from_slice(&(-12i32).to_le_bytes());
    lf[16..20].copy_from_slice(&400i32.to_le_bytes());
    lf[23] = 1; // DEFAULT_CHARSET
    for (i, c) in "Segoe UI".encode_utf16().enumerate() {
        lf[28 + 2 * i..30 + 2 * i].copy_from_slice(&c.to_le_bytes());
    }
    // SAFETY: the caller's 92 bytes.
    unsafe { core::ptr::copy_nonoverlapping(lf.as_ptr(), out, 92) };
}

fn put_u32(out: *mut u8, at: usize, v: u32) {
    // SAFETY: inside the caller's structure.
    unsafe { out.add(at).cast::<u32>().write_unaligned(v) };
}

const SPI_GETICONTITLELOGFONT: u32 = 0x1f;
const SPI_GETNONCLIENTMETRICS: u32 = 0x29;
const SPI_GETWORKAREA: u32 = 0x30;
const SPI_GETHIGHCONTRAST: u32 = 0x42;
const SPI_SETDESKWALLPAPER: u32 = 0x14;
const SPI_GETDESKWALLPAPER: u32 = 0x73;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn NtOpenProcessToken(process: usize, access: u32, out: *mut usize) -> u32;
    fn NtQueryInformationToken(
        token: usize,
        class: u32,
        buf: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> u32;
    fn NtOpenKey(out: *mut usize, access: u32, attributes: *const ObjectAttributes) -> u32;
    fn NtQueryValueKey(
        key: usize,
        name: *const UnicodeString,
        class: u32,
        buf: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> u32;
    fn NtClose(h: usize) -> u32;
    fn GalaDesktopAction(
        request: *const u8,
        len: u32,
        out: *mut u8,
        out_len: u32,
        returned: *mut u32,
    ) -> u32;
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum: u16,
    buffer: *const u16,
}

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root: usize,
    name: *const UnicodeString,
    attributes: u32,
    descriptor: usize,
    qos: usize,
}

/// `\REGISTRY\USER\<the token user's SID>\Control Panel\Desktop` into
/// `out`; its length (0 when the token cannot be read).
fn desktop_key(out: &mut [u16; 160]) -> usize {
    let mut n = 0;
    let mut put = |s: &[u8], n: &mut usize| {
        for &c in s {
            if *n < out.len() {
                out[*n] = c.into();
                *n += 1;
            }
        }
    };
    put(b"\\REGISTRY\\USER\\S-", &mut n);
    let (mut token, mut buf, mut ret) = (0usize, [0u8; 96], 0u32);
    // SAFETY: our handle, buffer and lengths; -1 is the process itself.
    let sid = unsafe {
        if NtOpenProcessToken(usize::MAX, 0x8 /* TOKEN_QUERY */, &mut token) != 0 {
            return 0;
        }
        let status =
            NtQueryInformationToken(token, 1 /* TokenUser */, buf.as_mut_ptr(), 96, &mut ret);
        NtClose(token);
        if status != 0 {
            return 0;
        }
        // TOKEN_USER: the SID's address first (within `buf`).
        let at = buf.as_ptr().cast::<usize>().read_unaligned() - buf.as_ptr() as usize;
        match buf.get(at..) {
            Some(sid) => sid,
            None => return 0,
        }
    };
    // SID: revision, sub-authority count, the 48-bit authority (big
    // endian), the sub-authorities (little endian).
    if sid.len() < 8 || sid.len() < 8 + 4 * usize::from(sid[1]) {
        return 0;
    }
    let dec = |v: u64| {
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
        (d, i)
    };
    let (d, i) = dec(sid[0].into());
    put(&d[i..], &mut n);
    put(b"-", &mut n);
    let (d, i) = dec(sid[2..8].iter().fold(0u64, |a, &b| a << 8 | u64::from(b)));
    put(&d[i..], &mut n);
    for k in 0..usize::from(sid[1]) {
        put(b"-", &mut n);
        let at = 8 + 4 * k;
        let (d, i) =
            dec(u32::from_le_bytes([sid[at], sid[at + 1], sid[at + 2], sid[at + 3]]).into());
        put(&d[i..], &mut n);
    }
    put(b"\\Control Panel\\Desktop", &mut n);
    n
}

/// SPI_GETDESKWALLPAPER: the "Wallpaper" value into the caller's `max`
/// UTF-16 units at `out` (empty when there is none); false when it does
/// not fit.
fn get_wallpaper(out: *mut u16, max: usize) -> bool {
    let mut path = [0u16; 160];
    let len = desktop_key(&mut path);
    let mut text = [0u16; 262];
    let mut got = 0;
    if len != 0 {
        let key_name = UnicodeString {
            length: 2 * len as u16,
            maximum: 2 * len as u16,
            buffer: path.as_ptr(),
        };
        let attributes = ObjectAttributes {
            length: size_of::<ObjectAttributes>() as u32,
            root: 0,
            name: &key_name,
            attributes: 0x40, // OBJ_CASE_INSENSITIVE
            descriptor: 0,
            qos: 0,
        };
        let value: [u16; 9] = [87, 97, 108, 108, 112, 97, 112, 101, 114]; // "Wallpaper"
        let value_name = UnicodeString { length: 18, maximum: 18, buffer: value.as_ptr() };
        // KEY_VALUE_PARTIAL_INFORMATION: title index, type, length, data.
        let mut info = [0u8; 12 + 2 * 262];
        let (mut key, mut ret) = (0usize, 0u32);
        // SAFETY: our structures, buffer and lengths.
        unsafe {
            if NtOpenKey(&mut key, 0x1 /* KEY_QUERY_VALUE */, &attributes) == 0 {
                let status = NtQueryValueKey(
                    key,
                    &value_name,
                    2,
                    info.as_mut_ptr(),
                    info.len() as u32,
                    &mut ret,
                );
                NtClose(key);
                let kind = u32::from_le_bytes([info[4], info[5], info[6], info[7]]);
                let bytes = u32::from_le_bytes([info[8], info[9], info[10], info[11]]) as usize;
                if status == 0 && (kind == 1 || kind == 2) {
                    for (k, u) in info[12..12 + bytes.min(2 * 262)].chunks_exact(2).enumerate() {
                        let u = u16::from_le_bytes([u[0], u[1]]);
                        if u == 0 {
                            break;
                        }
                        text[k] = u;
                        got = k + 1;
                    }
                }
            }
        }
    }
    if got + 1 > max {
        return false;
    }
    // SAFETY: the caller's buffer of `max` units.
    unsafe {
        core::ptr::copy_nonoverlapping(text.as_ptr(), out, got);
        out.add(got).write(0);
    }
    true
}

/// SPI_SETDESKWALLPAPER: the desktop shows picture `path` (NUL-ended;
/// empty: none) and saves it; the NTSTATUS.
fn set_wallpaper(path: *const u16) -> u32 {
    // GalaDesktopAction's request (kernel/src/nt/action.rs): version,
    // action, HWND, four arguments, the text's length, then the text.
    let mut b = [0u8; 40 + 2 * 260];
    b[0..4].copy_from_slice(&1u32.to_le_bytes());
    b[4..8].copy_from_slice(&10u32.to_le_bytes()); // ACTION_WALLPAPER
    let mut len = 0;
    // SAFETY: the caller's NUL-ended string, read up to MAX_PATH.
    unsafe {
        while len < 260 && *path.add(len) != 0 {
            b[40 + 2 * len..42 + 2 * len].copy_from_slice(&(*path.add(len)).to_le_bytes());
            len += 1;
        }
        if len == 260 {
            return 0xc000_000d; // STATUS_INVALID_PARAMETER
        }
    }
    if len == 0 {
        for (k, c) in b"drawn".iter().enumerate() {
            b[40 + 2 * k] = *c;
        }
        len = 5;
    }
    b[32..36].copy_from_slice(&(len as u32).to_le_bytes());
    let mut returned = 0u32;
    // SAFETY: our buffers and their lengths.
    unsafe {
        GalaDesktopAction(
            b.as_ptr(),
            (40 + 2 * len) as u32,
            core::ptr::null_mut(),
            0,
            &mut returned,
        )
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn SystemParametersInfoW(
    action: u32,
    param: u32,
    out: *mut u8,
    _update: u32,
) -> i32 {
    let fail = || {
        set_last_error(87); // ERROR_INVALID_PARAMETER
        0
    };
    // Booleans and numbers written as a BOOL/UINT.
    let value = match action {
        0x0026 => Some(1),          // SPI_GETDRAGFULLWINDOWS
        0x001b => Some(0),          // SPI_GETMENUDROPALIGNMENT
        0x0046 => Some(0),          // SPI_GETSCREENREADER
        0x004a => Some(1),          // SPI_GETFONTSMOOTHING
        0x005f => Some(0),          // SPI_GETSNAPTODEFBUTTON
        0x0062 | 0x0064 => Some(4), // SPI_GETMOUSEHOVERWIDTH/HEIGHT
        0x0066 => Some(400),        // SPI_GETMOUSEHOVERTIME
        0x0068 | 0x006c => Some(3), // SPI_GETWHEELSCROLLLINES/CHARS
        0x006a => Some(400),        // SPI_GETMENUSHOWDELAY
        0x1002 | 0x1004 | 0x1006 | 0x1012 | 0x1014 | 0x1016 | 0x1018 => Some(0), // animations, fades
        0x1008 => Some(1),          // SPI_GETGRADIENTCAPTIONS
        0x100a => Some(0),          // SPI_GETKEYBOARDCUES
        0x100e => Some(1),          // SPI_GETHOTTRACKING
        0x1022 => Some(1),          // SPI_GETFLATMENU
        0x1024 => Some(1),          // SPI_GETDROPSHADOW
        0x103e => Some(0),          // SPI_GETUIEFFECTS
        0x1042 => Some(0),          // SPI_GETCLIENTAREAANIMATION
        0x2006 => Some(1),          // SPI_GETCARETWIDTH
        0x200a => Some(2),          // SPI_GETFONTSMOOTHINGTYPE: ClearType
        0x200c => Some(1200),       // SPI_GETFONTSMOOTHINGCONTRAST
        0x200e | 0x2010 => Some(1), // SPI_GETFOCUSBORDERWIDTH/HEIGHT
        0x201c => Some(2),          // SPI_GETMOUSEWHEELROUTING (hybrid, the default)
        _ => None,
    };
    if let Some(v) = value {
        if out.is_null() {
            return fail();
        }
        put_u32(out, 0, v);
        return 1;
    }
    match action {
        SPI_GETWORKAREA => {
            if out.is_null() {
                return fail();
            }
            let (_, work) = crate::monitor::screen();
            for (k, v) in work.iter().enumerate() {
                put_u32(out, 4 * k, *v as u32);
            }
            1
        }
        SPI_GETICONTITLELOGFONT => {
            if out.is_null() || (param != 0 && param < 92) {
                return fail();
            }
            gui_font(out);
            1
        }
        SPI_GETNONCLIENTMETRICS => {
            // SAFETY: the caller's structure, cbSize first.
            let size =
                if out.is_null() { 0 } else { unsafe { out.cast::<u32>().read_unaligned() } };
            if size != 500 && size != 504 {
                return fail();
            }
            for (at, v) in
                [(4, 1u32), (8, 17), (12, 17), (16, CAPTION as u32), (20, CAPTION as u32)]
            {
                put_u32(out, at, v);
            }
            gui_font(out.wrapping_add(24));
            put_u32(out, 116, 20);
            put_u32(out, 120, 22);
            gui_font(out.wrapping_add(124));
            put_u32(out, 216, 19);
            put_u32(out, 220, 20);
            for at in [224, 316, 408] {
                gui_font(out.wrapping_add(at));
            }
            if size == 504 {
                put_u32(out, 500, 0);
            }
            1
        }
        SPI_GETDESKWALLPAPER => {
            if out.is_null() || !get_wallpaper(out.cast(), param as usize) {
                return fail();
            }
            1
        }
        SPI_SETDESKWALLPAPER => {
            if out.is_null() {
                // The default wallpaper again: gala keeps none apart.
                crate::window::unsupported(
                    b"user32.dll!SystemParametersInfoW(SPI_SETDESKWALLPAPER, NULL)",
                    0,
                );
                return fail();
            }
            match set_wallpaper(out.cast()) {
                0 => 1,
                0xc000_0034 | 0xc000_007b => {
                    set_last_error(2); // ERROR_FILE_NOT_FOUND
                    0
                }
                _ => fail(),
            }
        }
        SPI_GETHIGHCONTRAST => {
            // SAFETY: as above.
            let size =
                if out.is_null() { 0 } else { unsafe { out.cast::<u32>().read_unaligned() } };
            // HIGHCONTRAST: cbSize, dwFlags, lpszDefaultScheme (16 bytes on
            // x64, 12 on x86).
            if (size as usize) < 8 + size_of::<usize>() {
                return fail();
            }
            put_u32(out, 4, 0);
            // SAFETY: lpszDefaultScheme (NULL), after the two DWORDs.
            unsafe { out.add(8).cast::<usize>().write_unaligned(0) };
            1
        }
        _ => {
            crate::window::unsupported(
                b"user32.dll!SystemParametersInfoW(action)",
                u64::from(action),
            );
            fail()
        }
    }
}

/// SystemParametersInfoA: the same for actions without strings or fonts.
#[unsafe(no_mangle)]
pub extern "system" fn SystemParametersInfoA(
    action: u32,
    param: u32,
    out: *mut u8,
    update: u32,
) -> i32 {
    if matches!(
        action,
        SPI_GETICONTITLELOGFONT
            | SPI_GETNONCLIENTMETRICS
            | SPI_GETHIGHCONTRAST
            | SPI_GETDESKWALLPAPER
            | SPI_SETDESKWALLPAPER
    ) {
        crate::window::unsupported(b"user32.dll!SystemParametersInfoA(action)", u64::from(action));
        return 0;
    }
    SystemParametersInfoW(action, param, out, update)
}
