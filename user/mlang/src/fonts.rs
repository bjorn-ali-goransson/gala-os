//! The font side of mlang: the code pages of a font, and the font a
//! program is handed for a character the one it has cannot write.
//!
//! Every answer here comes out of gdi32: a font's code pages are the
//! FONTSIGNATURE of the face it realizes with (`GetTextCharsetInfo`,
//! `fsCsb[0]`), read in a DC the caller hands in, and a mapped font is
//! that DC's own font made again from its LOGFONTW -- a handle of the
//! program's own, which it may release or keep, and never the handle it
//! passed in (`mlang_test`).
//!
//! What gala cannot do is find another face: it ships one Latin family
//! (ARCHITECTURE "gdi32"), so a mask the DC's font does not already
//! cover has no face to map to and MapFont says so in the ledger and
//! answers E_FAIL. Windows would hand back a face from its font-link
//! list; a program that needs one will show up as that row.
//!
//! Handles mlang made are remembered on the object (`com`), so that
//! ReleaseFont deletes ours and leaves alone a font the program made
//! itself, and ResetFontMapping deletes every one still out. The
//! deletes happen with the object's lock let go (`com::take_fonts`),
//! because nothing of ours is held across a call into another DLL.

use core::ffi::c_void;

use crate::com::{self, Object};
use crate::{E_FAIL, E_OUTOFMEMORY, HResult, LEDGER_UNSUPPORTED, S_OK, put, reached, record};

/// GetObjectType's answer for a font (OBJ_FONT).
const OBJ_FONT: u32 = 6;
/// LOGFONTW: five LONGs, eight BYTEs and 32 WCHARs. Held as words,
/// because it starts with LONGs and gdi32 reads it aligned.
const LOGFONT_BYTES: i32 = 92;
const LOGFONT_WORDS: usize = LOGFONT_BYTES as usize / 4;
/// The code pages in a FONTSIGNATURE (`fsUsb[4]` then `fsCsb[2]`):
/// `fsCsb[0]` is the fifth word.
const FS_CSB0: usize = 4;

#[cfg_attr(
    target_arch = "x86",
    link(name = "gdi32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "gdi32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn SelectObject(dc: *mut c_void, object: *mut c_void) -> *mut c_void;
    fn GetObjectType(object: *mut c_void) -> u32;
    fn GetCurrentObject(dc: *mut c_void, kind: u32) -> *mut c_void;
    fn GetObjectW(object: *mut c_void, size: i32, out: *mut u8) -> i32;
    fn CreateFontIndirectW(logfont: *const u8) -> *mut c_void;
    fn DeleteObject(object: *mut c_void) -> i32;
    fn GetTextCharsetInfo(dc: *mut c_void, signature: *mut [u32; 6], flags: u32) -> u32;
}

/// The code pages of `font`, read in `dc`: the font is selected in, its
/// signature read and the DC's own font put back. `None` when `font` is
/// not a font or `dc` is not a DC, which is the E_FAIL every caller
/// answers with.
fn code_pages_of(dc: *mut c_void, font: *mut c_void) -> Option<u32> {
    // SAFETY: gdi32 answers for a handle that is not one (a NULL font
    // is no font, a handle that is not a DC takes nothing), so the
    // checks here are its own.
    unsafe {
        if GetObjectType(font) != OBJ_FONT {
            return None;
        }
        let old = SelectObject(dc, font);
        if old.is_null() {
            return None;
        }
        let mut signature = [0u32; 6];
        GetTextCharsetInfo(dc, &mut signature, 0);
        SelectObject(dc, old);
        Some(signature[FS_CSB0])
    }
}

/// GetFontCodePages(hdc, hFont, pdwCodePages).
pub fn get_font_code_pages(dc: *mut c_void, font: *mut c_void, out: *mut u32) -> HResult {
    put(out, 0);
    let Some(code_pages) = code_pages_of(dc, font) else { return E_FAIL };
    reached(crate::REACHED_FONT_CODE_PAGES, b"GetFontCodePages");
    put(out, code_pages);
    S_OK
}

/// MapFont(hdc, dwCodePages, chSrc, pFont): the DC's own font again,
/// when it covers the code pages asked for.
///
/// `chSrc` decides nothing here and goes to the ledger instead: gala's
/// one family covers the Latin code pages whole, so a character it
/// cannot write is a character no font gala has could write, and the
/// mask is the whole of what there is to answer from.
pub fn map_font(
    object: *mut Object,
    dc: *mut c_void,
    code_pages: u32,
    character: u16,
    out: *mut *mut c_void,
) -> HResult {
    put(out, core::ptr::null_mut());
    if code_pages == 0 {
        return E_FAIL;
    }
    // SAFETY: gdi32 answers NULL for a handle that is not a DC.
    let current = unsafe { GetCurrentObject(dc, OBJ_FONT) };
    let Some(covered) = code_pages_of(dc, current) else { return E_FAIL };
    if covered & code_pages != code_pages {
        record(
            b"MapFont(no face covers the code pages)",
            LEDGER_UNSUPPORTED,
            u64::from(code_pages & !covered),
        );
        return E_FAIL;
    }
    reached(crate::REACHED_MAP_FONT, b"MapFont");
    if out.is_null() {
        return S_OK;
    }
    let mut logfont = [0u32; LOGFONT_WORDS];
    // SAFETY: a LOGFONTW's worth of buffer for the font just read.
    let got = unsafe { GetObjectW(current, LOGFONT_BYTES, logfont.as_mut_ptr().cast::<u8>()) };
    if got != LOGFONT_BYTES {
        record(b"MapFont(the DC font has no LOGFONTW)", LEDGER_UNSUPPORTED, u64::from(character));
        return E_FAIL;
    }
    // SAFETY: the LOGFONTW read out of the DC's font.
    let mapped = unsafe { CreateFontIndirectW(logfont.as_ptr().cast::<u8>()) };
    if mapped.is_null() {
        return E_FAIL;
    }
    if !com::remember_font(object, mapped) {
        // SAFETY: a handle this call made and nobody else has.
        unsafe { DeleteObject(mapped) };
        return E_OUTOFMEMORY;
    }
    put(out, mapped);
    S_OK
}

/// ReleaseFont(hFont): a font mlang mapped is deleted, and one it never
/// mapped -- the program's own, or one already released -- is left as it
/// is. Either way S_OK; NULL is E_FAIL.
pub fn release_font(object: *mut Object, font: *mut c_void) -> HResult {
    if font.is_null() {
        return E_FAIL;
    }
    reached(crate::REACHED_RELEASE_FONT, b"ReleaseFont");
    if com::forget_font(object, font) {
        // SAFETY: a handle mlang made, now out of the object's table.
        unsafe { DeleteObject(font) };
    }
    S_OK
}

/// ResetFontMapping: every font mlang still has out is deleted.
pub fn reset_font_mapping(object: *mut Object) -> HResult {
    reached(crate::REACHED_RESET_FONT_MAPPING, b"ResetFontMapping");
    reset(object);
    S_OK
}

/// Deletes every font the object has mapped (ResetFontMapping, and the
/// last release of the object).
pub fn reset(object: *mut Object) {
    for font in com::take_fonts(object) {
        // SAFETY: a handle mlang made, now out of the object's table.
        unsafe { DeleteObject(font as *mut c_void) };
    }
}
