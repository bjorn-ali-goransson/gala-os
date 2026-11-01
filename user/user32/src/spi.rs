//! SystemParametersInfoW: gala's settings, with Windows' defaults where
//! gala has no choice of its own: the non-client metrics (a 1-pixel
//! border, the desktop's 30-pixel caption, 17-pixel scroll bars, the GUI
//! font: 12-pixel Segoe UI, which gdi32 draws in Noto Sans Mono), the work
//! area above the taskbar, no high contrast, three lines per wheel notch,
//! keyboard cues shown only after Alt, flat menus, UI effects off. Unknown
//! actions go to the ledger and fail.

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
        0x200e | 0x2010 => Some(1), // SPI_GETFOCUSBORDERWIDTH/HEIGHT
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
        SPI_GETHIGHCONTRAST => {
            // SAFETY: as above.
            let size =
                if out.is_null() { 0 } else { unsafe { out.cast::<u32>().read_unaligned() } };
            if size < 16 {
                return fail();
            }
            put_u32(out, 4, 0);
            // SAFETY: lpszDefaultScheme (NULL).
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
    if matches!(action, SPI_GETICONTITLELOGFONT | SPI_GETNONCLIENTMETRICS | SPI_GETHIGHCONTRAST) {
        crate::window::unsupported(b"user32.dll!SystemParametersInfoA(action)", u64::from(action));
        return 0;
    }
    SystemParametersInfoW(action, param, out, update)
}
