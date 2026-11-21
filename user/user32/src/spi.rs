//! SystemParametersInfoW: the settings a program asks the desktop for.
//! Every answer here is measured on a real Windows first (`spi_test`),
//! because a setting is handed to the caller and not refused: the
//! desktop's own sizes (the non-client metrics, the icon and minimized
//! metrics, the work area), its visual choices (animations, fades and UI
//! effects all on, as a Windows desktop answers; flat menus; ClearType),
//! its input settings (the keyboard's speed and delay, three lines per
//! wheel notch, the mouse's thresholds) and the accessibility ones (no
//! screen reader, no high contrast). Unknown actions go to the ledger
//! and fail, and every action a program is answered leaves a row saying
//! it read that setting.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::window::set_last_error;

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

const SPI_GETMOUSE: u32 = 0x03;
const SPI_GETICONTITLELOGFONT: u32 = 0x1f;
const SPI_GETNONCLIENTMETRICS: u32 = 0x29;
const SPI_GETMINIMIZEDMETRICS: u32 = 0x2b;
const SPI_GETICONMETRICS: u32 = 0x2d;
const SPI_GETWORKAREA: u32 = 0x30;
const SPI_GETHIGHCONTRAST: u32 = 0x42;

#[unsafe(no_mangle)]
pub extern "system" fn SystemParametersInfoW(
    action: u32,
    param: u32,
    out: *mut u8,
    update: u32,
) -> i32 {
    let ok = info(action, param, out, update);
    if ok != 0 {
        note_action(action, false);
    }
    ok
}

/// Every action a program is answered, once: the first answer for an
/// action leaves a "reached" ledger row naming it, and nothing after it
/// does.
///
/// This is what finds a divergence in a setting gala answers
/// successfully. A wrong answer is not refused and leaves nothing behind
/// -- the caller takes the path that setting calls for and goes on -- so
/// the only way to ask afterwards whether a setting mattered to a
/// program is to know that it read it. The ledger's `hits` then says how
/// often and its `first` and `last` when, against the rest of the run.
///
/// Bounded by construction: one row and one kernel call for an action
/// for the life of the process, at most SLOTS of them, and one row
/// saying so if a program asks for more distinct actions than that.
/// `ansi` keeps the two entries apart, since they are two names in the
/// ledger.
fn note_action(action: u32, ansi: bool) {
    const SLOTS: usize = 96;
    const ANSI: u32 = 0x8000_0000;
    static ASKED: [AtomicU32; SLOTS] = [const { AtomicU32::new(0) }; SLOTS];
    static FULL: AtomicBool = AtomicBool::new(false);
    // The action plus one, so that an empty slot is 0.
    let want = (action + 1) | if ansi { ANSI } else { 0 };
    let mut claimed = false;
    for slot in &ASKED {
        match slot.compare_exchange(0, want, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => {
                claimed = true;
                break;
            }
            // Asked before, by this entry.
            Err(seen) if seen == want => return,
            // Another action's slot.
            Err(_) => continue,
        }
    }
    if !claimed {
        if !FULL.swap(true, Ordering::Relaxed) {
            crate::window::unsupported(
                b"user32.dll!SystemParametersInfoW(actions): more than 96 asked",
                SLOTS as u64,
            );
        }
        return;
    }
    let mut name = [0u8; 48];
    let n = action_name(action, ansi, &mut name);
    crate::window::reached(&name[..n], u64::from(action));
}

/// `user32.dll!SystemParametersInfo[AW](0x<action>)` in `name`, its
/// length back. The action is in hex, which is how Windows documents it
/// and how its blocks (0x10xx, 0x20xx) read.
fn action_name(action: u32, ansi: bool, name: &mut [u8; 48]) -> usize {
    let mut n = 0;
    for &b in b"user32.dll!SystemParametersInfo" {
        name[n] = b;
        n += 1;
    }
    name[n] = if ansi { b'A' } else { b'W' };
    n += 1;
    for &b in b"(0x" {
        name[n] = b;
        n += 1;
    }
    let digits = (32 - action.max(1).leading_zeros()).div_ceil(4) as usize;
    for i in (0..digits).rev() {
        name[n] = b"0123456789abcdef"[((action >> (4 * i)) & 0xf) as usize];
        n += 1;
    }
    name[n] = b')';
    n + 1
}

fn info(action: u32, param: u32, out: *mut u8, _update: u32) -> i32 {
    let fail = || {
        set_last_error(87); // ERROR_INVALID_PARAMETER
        0
    };
    // Booleans and numbers written as a BOOL/UINT.
    let value = match action {
        0x0001 => Some(1),          // SPI_GETBEEP
        0x0005 => Some(1),          // SPI_GETBORDER
        0x000a => Some(31),         // SPI_GETKEYBOARDSPEED: the fastest
        0x0016 => Some(1),          // SPI_GETKEYBOARDDELAY
        0x0019 => Some(1),          // SPI_GETICONTITLEWRAP
        0x001b => Some(0),          // SPI_GETMENUDROPALIGNMENT
        0x0026 => Some(1),          // SPI_GETDRAGFULLWINDOWS
        0x0044 => Some(0),          // SPI_GETKEYBOARDPREF
        0x0046 => Some(0),          // SPI_GETSCREENREADER
        0x004a => Some(1),          // SPI_GETFONTSMOOTHING
        0x005f => Some(0),          // SPI_GETSNAPTODEFBUTTON
        0x0062 | 0x0064 => Some(4), // SPI_GETMOUSEHOVERWIDTH/HEIGHT
        0x0066 => Some(400),        // SPI_GETMOUSEHOVERTIME
        0x0068 | 0x006c => Some(3), // SPI_GETWHEELSCROLLLINES/CHARS
        0x006a => Some(400),        // SPI_GETMENUSHOWDELAY
        // The animations and the fades: a Windows desktop answers TRUE
        // for every one of them, and a program that asks whether
        // animations are on is asking what kind of desktop this is.
        0x1002 | 0x1004 | 0x1006 | 0x1012 | 0x1014 | 0x1016 | 0x1018 => Some(1),
        0x1008 => Some(1),          // SPI_GETGRADIENTCAPTIONS
        0x100a => Some(0),          // SPI_GETKEYBOARDCUES: shown after Alt
        0x100c => Some(0),          // SPI_GETACTIVEWNDTRKZORDER
        0x100e => Some(1),          // SPI_GETHOTTRACKING
        0x101a => Some(0),          // SPI_GETCURSORSHADOW
        0x101c => Some(0),          // SPI_GETMOUSESONAR
        0x101e => Some(0),          // SPI_GETMOUSECLICKLOCK
        0x1020 => Some(1),          // SPI_GETMOUSEVANISH
        0x1022 => Some(1),          // SPI_GETFLATMENU
        0x1024 => Some(1),          // SPI_GETDROPSHADOW
        0x1026 => Some(0),          // SPI_GETBLOCKSENDINPUTRESETS
        0x103e => Some(1),          // SPI_GETUIEFFECTS
        0x1040 => Some(0),          // SPI_GETDISABLEOVERLAPPEDCONTENT
        0x1042 => Some(1),          // SPI_GETCLIENTAREAANIMATION
        0x1044 | 0x1046 => Some(0), // answered 0 by the Windows machine
        // A machine's own setting rather than the system's, which is why
        // spi_test asks only that these two are answered: the lock
        // timeout in milliseconds and the number of taskbar flashes.
        0x2000 => Some(200_000),
        0x2004 => Some(3),
        0x2002 => Some(0),          // SPI_GETACTIVEWNDTRKTIMEOUT
        0x2006 => Some(1),          // SPI_GETCARETWIDTH
        0x200a => Some(2),          // SPI_GETFONTSMOOTHINGTYPE: ClearType
        0x200c => Some(1200),       // SPI_GETFONTSMOOTHINGCONTRAST
        0x200e | 0x2010 => Some(1), // SPI_GETFOCUSBORDERWIDTH/HEIGHT
        0x2012 => Some(1),          // SPI_GETFONTSMOOTHINGORIENTATION: RGB
        0x2014 => Some(0),          // SPI_GETMINIMUMHITRADIUS
        0x2016 => Some(5),          // SPI_GETMESSAGEDURATION, in seconds
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
        SPI_GETMOUSE => {
            if out.is_null() {
                return fail();
            }
            // The two thresholds and the acceleration, as Windows
            // answers them.
            for (k, v) in [6u32, 10, 1].iter().enumerate() {
                put_u32(out, 4 * k, *v);
            }
            1
        }
        SPI_GETMINIMIZEDMETRICS => {
            // SAFETY: the caller's structure, cbSize first.
            let size =
                if out.is_null() { 0 } else { unsafe { out.cast::<u32>().read_unaligned() } };
            if size != 20 {
                return fail();
            }
            // iWidth, no gaps, iArrange: ARW_HIDE.
            for (at, v) in [(4, 154u32), (8, 0), (12, 0), (16, 8)] {
                put_u32(out, at, v);
            }
            1
        }
        SPI_GETICONMETRICS => {
            // SAFETY: as above.
            let size =
                if out.is_null() { 0 } else { unsafe { out.cast::<u32>().read_unaligned() } };
            if size != 108 {
                return fail();
            }
            // The spacing of the desktop's icon grid, and titles wrapped.
            for (at, v) in [(4, 75u32), (8, 75), (12, 1)] {
                put_u32(out, at, v);
            }
            gui_font(out.wrapping_add(16));
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
            // Every size here is the system metric for the same
            // measurement, read from GetSystemMetrics rather than
            // repeated: that is the invariant a real Windows holds
            // (measured in spi_test), and two copies of a desktop's
            // sizes drift -- this structure said a caption button was
            // 32 wide while the metric said 46, a 19-pixel menu bar
            // 20 tall, and a small caption 20 wide against the
            // metric's 46. A program lays a window out from whichever
            // it asks.
            //
            // Windows' caption heights are a pixel less than the
            // metric, which counts the border below the caption;
            // gala's caption has no border of its own to subtract, so
            // the two are equal here, which is inside the relation.
            let m = |index: i32| crate::window::GetSystemMetrics(index) as u32;
            for (at, v) in [
                (4, m(5)),   // iBorderWidth, SM_CXBORDER
                (8, m(2)),   // iScrollWidth, SM_CXVSCROLL
                (12, m(3)),  // iScrollHeight, SM_CYHSCROLL
                (16, m(30)), // iCaptionWidth, SM_CXSIZE
                (20, m(4)),  // iCaptionHeight, SM_CYCAPTION
            ] {
                put_u32(out, at, v);
            }
            gui_font(out.wrapping_add(24));
            put_u32(out, 116, m(52)); // iSmCaptionWidth, SM_CXSMSIZE
            put_u32(out, 120, m(51)); // iSmCaptionHeight, SM_CYSMCAPTION
            gui_font(out.wrapping_add(124));
            put_u32(out, 216, m(54)); // iMenuWidth, SM_CXMENUSIZE
            put_u32(out, 220, m(55)); // iMenuHeight, SM_CYMENUSIZE
            for at in [224, 316, 408] {
                gui_font(out.wrapping_add(at));
            }
            if size == 504 {
                put_u32(out, 500, m(92)); // iPaddedBorderWidth
            }
            1
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
            | SPI_GETICONMETRICS
            | SPI_GETHIGHCONTRAST
    ) {
        crate::window::unsupported(b"user32.dll!SystemParametersInfoA(action)", u64::from(action));
        return 0;
    }
    let ok = info(action, param, out, update);
    if ok != 0 {
        note_action(action, true);
    }
    ok
}
