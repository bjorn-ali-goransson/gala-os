//! A DC's saved states (SaveDC/RestoreDC), its DC brush and pen colours,
//! the ANSI font calls, character widths, font data and font enumeration
//! (Chromium's chrome.dll delay-imports these). Behaviour as documented
//! on learn.microsoft.com (SaveDC, RestoreDC, SetDCBrushColor,
//! SetDCPenColor, CreateFontIndirectA, GetTextFaceA, GetCharWidthW,
//! GetFontData, EnumFontFamiliesExW).

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, Ordering};

use super::font::{self, LogFont, TextMetricW};
use super::{Dc, Handle, Object, slot_of, with_dc};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn MultiByteToWideChar(cp: u32, flags: u32, s: *const u8, n: i32, w: *mut u16, m: i32) -> i32;
    fn WideCharToMultiByte(
        cp: u32,
        flags: u32,
        w: *const u16,
        n: i32,
        s: *mut u8,
        m: i32,
        default: *const u8,
        used: *mut i32,
    ) -> i32;
}

const CLR_INVALID: u32 = 0xffff_ffff;
const GDI_ERROR: u32 = 0xffff_ffff;

// ---- Saved states ----

/// Every DC's saved states, oldest first: (the DC's slot, its state).
struct Saved {
    lock: AtomicBool,
    states: UnsafeCell<Vec<(usize, Dc)>>,
}

// SAFETY: every access is under `lock`.
unsafe impl Sync for Saved {}

static SAVED: Saved = Saved { lock: AtomicBool::new(false), states: UnsafeCell::new(Vec::new()) };

/// Runs `f` on the saved states (taken after the object table's lock
/// when both are held).
fn saved<R>(f: impl FnOnce(&mut Vec<(usize, Dc)>) -> R) -> R {
    while SAVED.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = f(unsafe { &mut *SAVED.states.get() });
    SAVED.lock.store(false, Ordering::Release);
    r
}

/// Drops a deleted DC's saved states.
pub(crate) fn forget(slot: usize) {
    saved(|states| states.retain(|(s, _)| *s != slot));
}

/// SaveDC: pushes the DC's state; the new depth (1 for the first), 0 on
/// failure.
#[unsafe(no_mangle)]
pub extern "system" fn SaveDC(hdc: Handle) -> i32 {
    let Some(slot) = slot_of(hdc) else { return 0 };
    with_dc(hdc, |dc, _| {
        saved(|states| {
            if states.try_reserve(1).is_err() {
                return 0;
            }
            states.push((slot, *dc));
            states.iter().filter(|(s, _)| *s == slot).count() as i32
        })
    })
    .unwrap_or(0)
}

/// RestoreDC(hdc, n): back to the state SaveDC returned `n` for (or the
/// `-n`th most recent for a negative `n`), dropping it and those saved
/// after it. FALSE for a depth that was not saved.
#[unsafe(no_mangle)]
pub extern "system" fn RestoreDC(hdc: Handle, n: i32) -> i32 {
    let Some(slot) = slot_of(hdc) else { return 0 };
    with_dc(hdc, |dc, _| {
        saved(|states| {
            let mine: Vec<usize> = states
                .iter()
                .enumerate()
                .filter(|(_, (s, _))| *s == slot)
                .map(|(i, _)| i)
                .collect();
            let depth = mine.len() as i32;
            let level = if n < 0 { depth + n + 1 } else { n };
            if n == 0 || level < 1 || level > depth {
                return 0;
            }
            let at = mine[level as usize - 1];
            *dc = states[at].1;
            let mut index = 0;
            states.retain(|(s, _)| {
                let keep = *s != slot || index < at;
                index += 1;
                keep
            });
            1
        })
    })
    .unwrap_or(0)
}

// ---- DC brush and pen colours ----

/// SetDCBrushColor: the DC_BRUSH colour; the previous one (CLR_INVALID
/// for a handle that is not a DC).
#[unsafe(no_mangle)]
pub extern "system" fn SetDCBrushColor(hdc: Handle, color: u32) -> u32 {
    with_dc(hdc, |dc, _| core::mem::replace(&mut dc.dc_brush, color & 0x00ff_ffff))
        .unwrap_or(CLR_INVALID)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetDCBrushColor(hdc: Handle) -> u32 {
    with_dc(hdc, |dc, _| dc.dc_brush).unwrap_or(CLR_INVALID)
}

/// SetDCPenColor: the DC_PEN colour; the previous one.
#[unsafe(no_mangle)]
pub extern "system" fn SetDCPenColor(hdc: Handle, color: u32) -> u32 {
    with_dc(hdc, |dc, _| core::mem::replace(&mut dc.dc_pen, color & 0x00ff_ffff))
        .unwrap_or(CLR_INVALID)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetDCPenColor(hdc: Handle) -> u32 {
    with_dc(hdc, |dc, _| dc.dc_pen).unwrap_or(CLR_INVALID)
}

// ---- ANSI fonts ----

/// LOGFONTA: LOGFONTW with an ANSI face name.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LogFontA {
    head: [i32; 5],
    bytes: [u8; 8],
    face: [u8; 32],
}

fn to_wide(lf: &LogFontA) -> LogFont {
    let mut face = [0u16; 32];
    let len = lf.face.iter().position(|&c| c == 0).unwrap_or(31).min(31);
    if len > 0 {
        // SAFETY: `len` bytes in, at most 31 units out (CP_ACP).
        unsafe { MultiByteToWideChar(0, 0, lf.face.as_ptr(), len as i32, face.as_mut_ptr(), 31) };
    }
    let [height, width, escapement, orientation, weight] = lf.head;
    let b = lf.bytes;
    LogFont {
        height,
        width,
        escapement,
        orientation,
        weight,
        italic: b[0],
        underline: b[1],
        strike_out: b[2],
        char_set: b[3],
        out_precision: b[4],
        clip_precision: b[5],
        quality: b[6],
        pitch_and_family: b[7],
        face,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateFontIndirectA(lf: *const LogFontA) -> Handle {
    if lf.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: the caller's LOGFONTA.
    let w = to_wide(unsafe { &*lf });
    super::CreateFontIndirectW(&w)
}

/// CreateFontA: a LOGFONTA from its fields.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn CreateFontA(
    height: i32,
    width: i32,
    escapement: i32,
    orientation: i32,
    weight: i32,
    italic: u32,
    underline: u32,
    strike_out: u32,
    char_set: u32,
    out_precision: u32,
    clip_precision: u32,
    quality: u32,
    pitch_and_family: u32,
    face: *const u8,
) -> Handle {
    let mut lf = LogFontA {
        head: [height, width, escapement, orientation, weight],
        bytes: [
            italic as u8,
            underline as u8,
            strike_out as u8,
            char_set as u8,
            out_precision as u8,
            clip_precision as u8,
            quality as u8,
            pitch_and_family as u8,
        ],
        face: [0; 32],
    };
    if !face.is_null() {
        for i in 0..31 {
            // SAFETY: the caller's NUL-terminated name, up to 31 bytes.
            let c = unsafe { *face.add(i) };
            if c == 0 {
                break;
            }
            lf.face[i] = c;
        }
    }
    CreateFontIndirectA(&lf)
}

fn font_of(dc: &Dc, slots: &[Object]) -> LogFont {
    match slots[dc.font] {
        Object::Font(f) => f,
        _ => LogFont::default_gui(),
    }
}

/// GetTextFaceA: the selected font's face name in the ANSI code page; the
/// bytes copied with the NUL (the size needed when `buf` is NULL).
#[unsafe(no_mangle)]
pub extern "system" fn GetTextFaceA(hdc: Handle, n: i32, buf: *mut u8) -> i32 {
    with_dc(hdc, |dc, slots| {
        let f = font_of(dc, slots);
        let len = f.face.iter().position(|&c| c == 0).unwrap_or(f.face.len());
        let mut ansi = [0u8; 64];
        // SAFETY: `len` units in, our buffer out (CP_ACP).
        let k = unsafe {
            WideCharToMultiByte(
                0,
                0,
                f.face.as_ptr(),
                len as i32,
                ansi.as_mut_ptr(),
                63,
                core::ptr::null(),
                core::ptr::null_mut(),
            )
        }
        .max(0) as usize;
        if buf.is_null() {
            return k as i32 + 1;
        }
        let k = k.min((n.max(1) - 1) as usize);
        // SAFETY: the caller's buffer of `n` bytes.
        unsafe {
            core::ptr::copy_nonoverlapping(ansi.as_ptr(), buf, k);
            *buf.add(k) = 0;
        }
        k as i32 + 1
    })
    .unwrap_or(0)
}

// ---- Widths and font data ----

/// GetCharWidthW(hdc, first, last, widths): each character's advance in
/// logical units, `last - first + 1` of them; FALSE when `last < first`.
#[unsafe(no_mangle)]
pub extern "system" fn GetCharWidthW(hdc: Handle, first: u32, last: u32, out: *mut i32) -> i32 {
    if out.is_null() || last < first {
        return 0;
    }
    with_dc(hdc, |dc, slots| {
        let m = font::metrics(&font_of(dc, slots));
        for (i, code) in (first..=last).enumerate() {
            let w = char::from_u32(code).map_or(0, |c| m.advance(c));
            // SAFETY: the caller's array of `last - first + 1` widths.
            unsafe { out.add(i).write(w) };
        }
        1
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetCharWidth32W(hdc: Handle, first: u32, last: u32, out: *mut i32) -> i32 {
    GetCharWidthW(hdc, first, last, out)
}

/// The bytes of table `tag` (as GetFontData names it: the tag's bytes in
/// memory order; 0 for the whole file) of a TrueType file.
fn table(file: &'static [u8], tag: u32) -> Option<&'static [u8]> {
    if tag == 0 {
        return Some(file);
    }
    let be16 = |at: usize| Some(u16::from_be_bytes([*file.get(at)?, *file.get(at + 1)?]));
    let be32 = |at: usize| {
        Some(u32::from_be_bytes([
            *file.get(at)?,
            *file.get(at + 1)?,
            *file.get(at + 2)?,
            *file.get(at + 3)?,
        ]))
    };
    let want = tag.to_le_bytes();
    for i in 0..usize::from(be16(4)?) {
        let record = 12 + 16 * i;
        if file.get(record..record + 4)? == want {
            let (offset, length) = (be32(record + 8)? as usize, be32(record + 12)? as usize);
            return file.get(offset..offset.checked_add(length)?);
        }
    }
    None
}

/// GetFontData(hdc, table, offset, buf, size): bytes of the selected
/// font's file from `offset` in `table` (0: the whole file); the count
/// copied, or the size from `offset` when `buf` is NULL or `size` 0.
/// GDI_ERROR for a missing table, an offset past it, or a font without a
/// file (gala's fixed-pitch bitmap font).
#[unsafe(no_mangle)]
pub extern "system" fn GetFontData(
    hdc: Handle,
    tag: u32,
    offset: u32,
    buf: *mut c_void,
    size: u32,
) -> u32 {
    with_dc(hdc, |dc, slots| {
        let Some(bytes) = font::file(&font_of(dc, slots)).and_then(|f| table(f, tag)) else {
            return GDI_ERROR;
        };
        let Some(rest) = bytes.get(offset as usize..) else { return GDI_ERROR };
        if buf.is_null() || size == 0 {
            return rest.len() as u32;
        }
        let n = rest.len().min(size as usize);
        // SAFETY: the caller's buffer of `size` bytes.
        unsafe { core::ptr::copy_nonoverlapping(rest.as_ptr(), buf.cast::<u8>(), n) };
        n as u32
    })
    .unwrap_or(GDI_ERROR)
}

// ---- Enumeration ----

/// ENUMLOGFONTEXW: the LOGFONTW, the full name, the style, the script.
#[repr(C)]
pub struct EnumLogFontExW {
    font: LogFont,
    full_name: [u16; 64],
    style: [u16; 32],
    script: [u16; 32],
}

/// NEWTEXTMETRICEXW: TEXTMETRICW, its TrueType fields, FONTSIGNATURE.
#[repr(C)]
pub struct NewTextMetricExW {
    metric: TextMetricW,
    flags: u32,
    size_em: u32,
    cell_height: u32,
    avg_width: u32,
    signature: [u32; 6],
}

type FontEnumProcW =
    extern "system" fn(*const EnumLogFontExW, *const NewTextMetricExW, u32, isize) -> i32;

const TRUETYPE_FONTTYPE: u32 = 4;
const NTM_REGULAR: u32 = 0x40;
const NTM_BOLD: u32 = 0x20;

fn put_str(out: &mut [u16], parts: &[&str]) {
    let room = out.len().saturating_sub(1);
    for (slot, c) in out[..room].iter_mut().zip(parts.iter().flat_map(|p| p.encode_utf16())) {
        *slot = c;
    }
}

/// EnumFontFamiliesExW: the installed families matching the LOGFONTW's
/// face name and character set. gala has one, Selawik (in four weights):
/// an empty name enumerates it once (its Regular face), "Selawik" each of
/// its faces; other names have no fonts (no callback). Selawik covers
/// ANSI_CHARSET; DEFAULT_CHARSET means any. The last callback's value,
/// or 1 when there was none.
#[unsafe(no_mangle)]
pub extern "system" fn EnumFontFamiliesExW(
    hdc: Handle,
    filter: *const LogFont,
    callback: Option<FontEnumProcW>,
    param: isize,
    _flags: u32,
) -> i32 {
    let (Some(callback), false) = (callback, filter.is_null()) else { return 0 };
    if with_dc(hdc, |_, _| ()).is_none() {
        return 0;
    }
    // SAFETY: the caller's LOGFONTW.
    let filter = unsafe { &*filter };
    if !matches!(filter.char_set, 0 | 1) {
        return 1;
    }
    let len = filter.face.iter().position(|&c| c == 0).unwrap_or(32);
    let name = &filter.face[..len];
    let lower = |c: u16| if (0x41..=0x5a).contains(&c) { c + 0x20 } else { c };
    let is_selawik =
        name.len() == 7 && name.iter().zip(b"selawik").all(|(&a, &b)| lower(a) == u16::from(b));
    if !name.is_empty() && !is_selawik {
        return 1;
    }
    let mut result = 1;
    for (file, style, weight) in font::selawik_faces() {
        // head's unitsPerEm (at 18).
        let em = table(file, u32::from_le_bytes(*b"head"))
            .and_then(|h| Some(u32::from(u16::from_be_bytes([*h.get(18)?, *h.get(19)?]))))
            .unwrap_or(2048);
        if name.is_empty() && weight != 400 {
            continue;
        }
        let mut face = [0u16; 32];
        put_str(&mut face, &["Selawik"]);
        let mut lf = LogFont { weight, char_set: 0, pitch_and_family: 0x22, face, ..*filter };
        lf.height = -12;
        lf.width = 0;
        lf.italic = 0;
        lf.quality = 0;
        let mut e =
            EnumLogFontExW { font: lf, full_name: [0; 64], style: [0; 32], script: [0; 32] };
        if weight == 400 {
            put_str(&mut e.full_name, &["Selawik"]);
        } else {
            put_str(&mut e.full_name, &["Selawik ", style]);
        }
        put_str(&mut e.style, &[style]);
        put_str(&mut e.script, &["Western"]);
        let metric = font::text_metrics(&lf);
        let m = NewTextMetricExW {
            avg_width: metric.ave_char_width as u32,
            cell_height: metric.height as u32,
            metric,
            flags: if weight >= 700 { NTM_BOLD } else { NTM_REGULAR },
            size_em: em,
            // Latin 1 (bit 0 of the Unicode ranges' first word), code page
            // 1252 (bit 0 of the ANSI code pages).
            signature: [1, 0, 0, 0, 1, 0],
        };
        result = callback(&e, &m, TRUETYPE_FONTTYPE, param);
        if result == 0 {
            break;
        }
    }
    result
}
