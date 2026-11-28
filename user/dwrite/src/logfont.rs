//! A font as GDI knows it (LOGFONTW, wingdi.h):
//! IDWriteGdiInterop::ConvertFontToLOGFONT and ConvertFontFaceToLOGFONT,
//! which a program calls when it holds a DirectWrite font and has to
//! name it to the GDI half of Windows -- to measure with a device
//! context, to put it in a font dialog, or to draw with gdi32 after
//! laying the text out with DirectWrite. Word asks on the way to its
//! first window.
//!
//! The LOGFONT is of a font and not of a size (measured on the pinned
//! Windows, `dwrite_test` step 13): the height, the width, the
//! escapement and the orientation are all zero, because a DirectWrite
//! font has no size of its own -- the em size belongs to the text
//! format, and a program that wants one fills the height in itself.
//! What the conversion fills in is the font: its weight, whether it is
//! italic, DEFAULT_CHARSET, OUT_OUTLINE_PRECIS, and the family's name as
//! the face name, which is the name GDI looks a font up by (so gala
//! writes Selawik where Windows writes Arial, each the name its own
//! collection answered with).
//!
//! A refused conversion clears the whole structure first, as Windows'
//! does: nothing of a font a program was not given can be read back out
//! of it.

use core::ffi::c_void;

use crate::{E_INVALIDARG, E_POINTER, HResult, S_OK, answered, font, missing};

/// LF_FACESIZE: the characters a LOGFONT's face name has room for,
/// terminator included.
const FACE_SIZE: usize = 32;
/// DEFAULT_CHARSET.
const DEFAULT_CHARSET: u8 = 1;
/// OUT_OUTLINE_PRECIS: the font is an outline font, which is what GDI is
/// told to pick when it looks this LOGFONT up again.
const OUT_OUTLINE_PRECIS: u8 = 8;

/// LOGFONTW, the structure the caller hands us to fill in.
#[repr(C)]
pub struct LogFont {
    height: i32,
    width: i32,
    escapement: i32,
    orientation: i32,
    weight: i32,
    italic: u8,
    underline: u8,
    strike_out: u8,
    charset: u8,
    out_precision: u8,
    clip_precision: u8,
    quality: u8,
    pitch_and_family: u8,
    face_name: [u16; FACE_SIZE],
}

impl LogFont {
    const EMPTY: LogFont = LogFont {
        height: 0,
        width: 0,
        escapement: 0,
        orientation: 0,
        weight: 0,
        italic: 0,
        underline: 0,
        strike_out: 0,
        charset: 0,
        out_precision: 0,
        clip_precision: 0,
        quality: 0,
        pitch_and_family: 0,
        face_name: [0; FACE_SIZE],
    };
}

/// The structure is Windows', so its size is part of the ABI and not
/// ours to choose: a program allocates it and we write into it.
const _: () = assert!(core::mem::size_of::<LogFont>() == 92);

/// ConvertFontToLOGFONT(font, logFont, &isSystemFont) (4). Every font of
/// gala's is of the system collection, so the system flag is always set.
pub extern "system" fn interop_ConvertFontToLOGFONT(
    _this: *const c_void,
    font_obj: *const c_void,
    out: *mut LogFont,
    is_system: *mut i32,
) -> HResult {
    let r = if is_system.is_null() {
        E_POINTER
    } else {
        let r = convert(font_obj, font::index_of_font(font_obj), out);
        // SAFETY: the caller's output, not NULL.
        unsafe { *is_system = i32::from(r == S_OK) };
        r
    };
    answered(b"IDWriteGdiInterop::ConvertFontToLOGFONT", r);
    r
}

/// ConvertFontFaceToLOGFONT(font, logFont) (5): the same LOGFONT for a
/// face, which has no system flag -- a face is one either way.
pub extern "system" fn interop_ConvertFontFaceToLOGFONT(
    _this: *const c_void,
    face: *const c_void,
    out: *mut LogFont,
) -> HResult {
    let r = convert(face, font::font_of_face(face), out);
    answered(b"IDWriteGdiInterop::ConvertFontFaceToLOGFONT", r);
    r
}

/// Fills the caller's LOGFONT in for the font at `index`, or clears it
/// and refuses. `object` is what the caller named the font with, which
/// the ledger wants when it is a font of someone else's.
fn convert(object: *const c_void, index: Option<usize>, out: *mut LogFont) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // Cleared first: a refusal leaves nothing of a font in a structure
    // the program will read (measured -- Windows clears it too).
    // SAFETY: the caller's structure.
    unsafe { core::ptr::write(out, LogFont::EMPTY) };
    let Some(i) = index else {
        if !object.is_null() {
            missing(b"dwrite.dll!ConvertFontToLOGFONT: a font of another collection", 0);
        }
        return E_INVALIDARG;
    };
    let (weight, italic, name) = font::as_logfont(i);
    let mut face_name = [0u16; FACE_SIZE];
    for (to, &c) in face_name.iter_mut().zip(name).take(FACE_SIZE - 1) {
        *to = c;
    }
    // SAFETY: the caller's structure, as above.
    unsafe {
        core::ptr::write(
            out,
            LogFont {
                weight: weight as i32,
                italic: u8::from(italic),
                charset: DEFAULT_CHARSET,
                out_precision: OUT_OUTLINE_PRECIS,
                face_name,
                ..LogFont::EMPTY
            },
        );
    }
    S_OK
}
