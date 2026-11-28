//! Text formats (IDWriteTextFormat, dwrite.h): the family, the weight,
//! the style, the stretch, the em size and the locale a program lays a
//! piece of text out with, and the alignment, wrapping, direction, tab
//! stop, line spacing and trimming of the paragraph it lays it into.
//! Word's start-up asks for one per piece of text its window is made of
//! -- 279 of them in the tenth of a second before the window appears --
//! and reads back what it set.
//!
//! A format is not a lookup and not a font: it keeps the family name and
//! the locale name it was made with, whatever they are, and finding a
//! face for them is the text layout's work. So a family no collection
//! has, an empty name and a thousand characters are all kept verbatim,
//! and what a create refuses is only what it can tell on its own: a NULL
//! name, a weight outside 1..=999, a style or stretch past its
//! enumeration, a size that is not a positive number, and a locale name
//! that is not BCP-47's shape (measured on the pinned Windows,
//! `dwrite_test` step 13).
//!
//! Each format is a counted object of its own, as a program holds it
//! past the call that made it and sets the paragraph's values on it
//! later. The later revisions (IDWriteTextFormat1, 2 and 3) are refused
//! as the factory's are and for the same reason (`FACTORY_IIDS`), but
//! their slots are in the vtable, so a program that casts without asking
//! records a method's name instead of calling a word past the end of the
//! table.
//!
//! Typography (IDWriteTypography) and number substitution
//! (IDWriteNumberSubstitution) are the other two objects a format's text
//! is laid out with, and they live here beside it: a set of font features
//! a program adds one at a time, and the token that says which digits a
//! locale's numbers are written in.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use alloc::vec::Vec;

use crate::{
    E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, HResult, Iid, S_OK, VTable, answered, font, guid,
    query, reached, vt,
};

pub const IID_TEXT_FORMAT: Iid =
    guid(0x9c90_6818, 0x31d7, 0x4fd3, [0xa1, 0x51, 0x7c, 0x5e, 0x22, 0x5d, 0xb5, 0x5a]);
pub const IID_TYPOGRAPHY: Iid =
    guid(0x55f1_112b, 0x1dc2, 0x4b3c, [0x95, 0x41, 0xf4, 0x68, 0x94, 0xed, 0x85, 0xb6]);
pub const IID_NUMBER_SUBSTITUTION: Iid =
    guid(0x1488_5cc9, 0xbab0, 0x4f90, [0xb6, 0xed, 0x5c, 0x36, 0x6a, 0x2c, 0xd0, 0x3d]);

/// ERROR_INSUFFICIENT_BUFFER as an HRESULT, which is what a string
/// getter says when the buffer has no room for the terminator too.
pub(crate) const E_NOT_SUFFICIENT_BUFFER: HResult = 0x8007_007a_u32 as i32;

/// The weights DirectWrite takes: 1 to 999, which is wider than
/// DWRITE_FONT_WEIGHT's own values and keeps whatever is given (450 is
/// taken and read back) -- measured, where 0 and 1000 are E_INVALIDARG.
pub(crate) const WEIGHT_MIN: u32 = 1;
pub(crate) const WEIGHT_MAX: u32 = 999;
/// The last DWRITE_FONT_STYLE (ITALIC).
pub(crate) const STYLE_LAST: u32 = 2;
/// DWRITE_FONT_STRETCH's values, 1 (ULTRA_CONDENSED) to 9
/// (ULTRA_EXPANDED); UNDEFINED (0) is refused.
pub(crate) const STRETCH_MIN: u32 = 1;
pub(crate) const STRETCH_MAX: u32 = 9;
/// The last DWRITE_TEXT_ALIGNMENT (JUSTIFIED).
const TEXT_ALIGNMENT_LAST: u32 = 3;
/// The last DWRITE_PARAGRAPH_ALIGNMENT (CENTER).
const PARAGRAPH_ALIGNMENT_LAST: u32 = 2;
/// The last DWRITE_WORD_WRAPPING (CHARACTER).
const WORD_WRAPPING_LAST: u32 = 4;
/// The last DWRITE_READING_DIRECTION (BOTTOM_TO_TOP).
const READING_DIRECTION_LAST: u32 = 3;
/// The last DWRITE_FLOW_DIRECTION (RIGHT_TO_LEFT).
const FLOW_DIRECTION_LAST: u32 = 3;
/// The last DWRITE_LINE_SPACING_METHOD (PROPORTIONAL).
const LINE_SPACING_LAST: u32 = 2;
/// The last DWRITE_TRIMMING_GRANULARITY (WORD).
const TRIMMING_LAST: u32 = 2;
/// The longest run between the hyphens of a locale name (BCP-47's
/// subtag): a run of eight is taken, nine is E_INVALIDARG, and two runs
/// of eight are taken -- so the rule is the run and not the name.
const LOCALE_RUN_MAX: usize = 8;
/// A format's incremental tab stop, which Windows sets to four times the
/// em size (measured at 16, 10, 7.5 and 0.5).
const TAB_STOPS_PER_EM: f32 = 4.0;
/// The last DWRITE_NUMBER_SUBSTITUTION_METHOD (TRADITIONAL).
const SUBSTITUTION_LAST: u32 = 4;
/// DWRITE_NUMBER_SUBSTITUTION_METHOD_NONE, the one method that takes no
/// locale at all: the others have a locale's digits to substitute from
/// (measured -- with a NULL locale they are E_INVALIDARG).
const SUBSTITUTION_NONE: u32 = 2;

/// What a format holds and a program sets on it, and what a text layout
/// copies out of the format it was made with. The strings are not here:
/// they are the two fields beside this one in `Format`, because they
/// have no length limit of their own (a thousand characters is taken) --
/// a format keeps them in its own allocation, a layout in vectors.
#[derive(Clone, Copy)]
pub(crate) struct Values {
    pub(crate) weight: u32,
    pub(crate) style: u32,
    pub(crate) stretch: u32,
    pub(crate) size: f32,
    pub(crate) text_alignment: u32,
    pub(crate) paragraph_alignment: u32,
    pub(crate) wrapping: u32,
    pub(crate) reading: u32,
    pub(crate) flow: u32,
    pub(crate) tab_stop: f32,
    pub(crate) spacing_method: u32,
    pub(crate) spacing: f32,
    pub(crate) baseline: f32,
    /// DWRITE_TRIMMING: the granularity, the delimiter and the count.
    pub(crate) trimming: [u32; 3],
    /// The trimming sign, an inline object of the caller's, held with a
    /// reference of our own.
    pub(crate) sign: *const c_void,
    /// The collection the format was made with, or NULL for the
    /// system's (which is what a NULL collection means).
    pub(crate) collection: *const c_void,
}

/// A text format: the values and the two strings, each a pointer and a
/// length. A format of its own keeps them NUL-terminated in the same
/// allocation after it; a text layout, which is a text format as well,
/// keeps this same prefix at its start and points these at strings of
/// its own, so that the 25 methods below answer for either (`layout`).
#[repr(C)]
pub(crate) struct Format {
    pub(crate) vtbl: *const (),
    pub(crate) refs: AtomicU32,
    pub(crate) values: Values,
    pub(crate) family: (*const u16, usize),
    pub(crate) locale: (*const u16, usize),
}

static FORMAT_VTBL: VTable<43> = VTable([
    format_qi as *const (),
    format_add_ref as *const (),
    format_release as *const (),
    format_SetTextAlignment as *const (),
    format_SetParagraphAlignment as *const (),
    format_SetWordWrapping as *const (),
    format_SetReadingDirection as *const (),
    format_SetFlowDirection as *const (),
    format_SetIncrementalTabStop as *const (),
    format_SetTrimming as *const (),
    format_SetLineSpacing as *const (),
    format_GetTextAlignment as *const (),
    format_GetParagraphAlignment as *const (),
    format_GetWordWrapping as *const (),
    format_GetReadingDirection as *const (),
    format_GetFlowDirection as *const (),
    format_GetIncrementalTabStop as *const (),
    format_GetTrimming as *const (),
    format_GetLineSpacing as *const (),
    format_GetFontCollection as *const (),
    format_GetFontFamilyNameLength as *const (),
    format_GetFontFamilyName as *const (),
    format_GetFontWeight as *const (),
    format_GetFontStyle as *const (),
    format_GetFontStretch as *const (),
    format_GetFontSize as *const (),
    format_GetLocaleNameLength as *const (),
    format_GetLocaleName as *const (),
    // IDWriteTextFormat1, 2 and 3, in the order the headers declare
    // them: refused by QueryInterface (as the factory's revisions are),
    // and here so that a program which casts to one without asking
    // records the method it wanted instead of calling a word past the
    // end of the table. The getters among them answer a value and not
    // an HRESULT, so each is the shape its header gives.
    format1_SetVerticalGlyphOrientation as *const (),
    format1_GetVerticalGlyphOrientation as *const (),
    format1_SetLastLineWrapping as *const (),
    format1_GetLastLineWrapping as *const (),
    format1_SetOpticalAlignment as *const (),
    format1_GetOpticalAlignment as *const (),
    format1_SetFontFallback as *const (),
    format1_GetFontFallback as *const (),
    format2_SetLineSpacing as *const (),
    format2_GetLineSpacing as *const (),
    format3_SetFontAxisValues as *const (),
    format3_GetFontAxisValueCount as *const (),
    format3_GetFontAxisValues as *const (),
    format3_GetAutomaticFontAxes as *const (),
    format3_SetAutomaticFontAxes as *const (),
]);

/// IDWriteFactory::CreateTextFormat(familyName, collection, weight,
/// style, stretch, size, localeName, &format) (15).
#[allow(clippy::too_many_arguments)]
pub extern "system" fn factory_CreateTextFormat(
    _this: *const c_void,
    family: *const u16,
    collection: *const c_void,
    weight: u32,
    style: u32,
    stretch: u32,
    size: f32,
    locale: *const u16,
    out: *mut *const c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's output.
    unsafe { *out = core::ptr::null() };
    let refused = family.is_null()
        || locale.is_null()
        || !(WEIGHT_MIN..=WEIGHT_MAX).contains(&weight)
        || style > STYLE_LAST
        || !(STRETCH_MIN..=STRETCH_MAX).contains(&stretch)
        || size.is_nan()
        || size <= 0.0
        || !locale_shaped(locale);
    if refused {
        answered(b"IDWriteFactory::CreateTextFormat", E_INVALIDARG);
        return E_INVALIDARG;
    }
    let values = Values {
        weight,
        style,
        stretch,
        size,
        text_alignment: 0,
        paragraph_alignment: 0,
        wrapping: 0,
        reading: 0,
        flow: 0,
        tab_stop: TAB_STOPS_PER_EM * size,
        spacing_method: 0,
        spacing: 0.0,
        baseline: 0.0,
        trimming: [0; 3],
        sign: core::ptr::null(),
        collection,
    };
    let r = make(values, family, locale, out);
    answered(b"IDWriteFactory::CreateTextFormat", r);
    r
}

/// A fresh format for the caller, with the two strings copied after it
/// (Windows hands out a new object per call: two creates of the same
/// values are two pointers).
fn make(
    values: Values,
    family: *const u16,
    locale: *const u16,
    out: *mut *const c_void,
) -> HResult {
    let family_len = wide_len(family);
    let locale_len = wide_len(locale);
    let Some(layout) = format_alloc(family_len, locale_len) else {
        return E_OUTOFMEMORY;
    };
    // SAFETY: a non-zero-sized layout.
    let p = unsafe { alloc::alloc::alloc(layout) }.cast::<Format>();
    if p.is_null() {
        return E_OUTOFMEMORY;
    }
    hold(values.collection);
    // SAFETY: fresh memory for one Format and the two strings after it.
    unsafe {
        let text = p.add(1).cast::<u16>();
        copy_wide(family, text, family_len);
        copy_wide(locale, text.add(family_len + 1), locale_len);
        p.write(Format {
            vtbl: vt(&FORMAT_VTBL),
            refs: AtomicU32::new(1),
            values,
            family: (text, family_len),
            locale: (text.add(family_len + 1), locale_len),
        });
    }
    // SAFETY: the caller's output.
    unsafe { *out = p.cast() };
    S_OK
}

/// The allocation one format needs: the object, then the two strings
/// with their terminators. None if the two lengths do not fit a layout
/// at all (a string long enough to overflow it).
fn format_alloc(family_len: usize, locale_len: usize) -> Option<core::alloc::Layout> {
    let text = family_len.checked_add(1)?.checked_add(locale_len)?.checked_add(1)?;
    let size = core::mem::size_of::<Format>().checked_add(text.checked_mul(2)?)?;
    core::alloc::Layout::from_size_align(size, core::mem::align_of::<Format>()).ok()
}

/// The characters of a NUL-terminated string the caller gave us.
pub(crate) fn wide_len(s: *const u16) -> usize {
    let mut n = 0;
    // SAFETY: the caller's string, which is NUL-terminated as every
    // Win32 string argument is.
    while unsafe { *s.add(n) } != 0 {
        n += 1;
    }
    n
}

/// Copies `n` characters and a terminator.
///
/// # Safety
/// `from` has `n` characters; `to` has room for `n + 1`.
pub(crate) unsafe fn copy_wide(from: *const u16, to: *mut u16, n: usize) {
    // SAFETY: as the caller promises.
    unsafe {
        core::ptr::copy_nonoverlapping(from, to, n);
        *to.add(n) = 0;
    }
}

/// Whether a locale name has the shape DirectWrite checks for, which is
/// BCP-47's and is about the runs and not the length (measured): runs of
/// one to eight characters with single hyphens between them, so no
/// leading or trailing hyphen and none doubled. What is inside a run is
/// never looked at -- "zz-not-a-locale" and "en_us" are both taken --
/// and an empty name is taken as well.
pub(crate) fn locale_shaped(locale: *const u16) -> bool {
    let mut run = 0usize;
    let mut any = false;
    let mut i = 0usize;
    loop {
        // SAFETY: the caller's NUL-terminated string.
        let c = unsafe { *locale.add(i) };
        if c == 0 {
            break;
        }
        any = true;
        if c == u16::from(b'-') {
            if run == 0 || run > LOCALE_RUN_MAX {
                return false;
            }
            run = 0;
        } else {
            run += 1;
        }
        i += 1;
    }
    !any || (1..=LOCALE_RUN_MAX).contains(&run)
}

/// Method `slot` of a COM object the caller gave us.
///
/// # Safety
/// `o` is a COM object whose vtable has that slot with that type.
pub(crate) unsafe fn method<F: Copy>(o: *const c_void, slot: usize) -> F {
    // SAFETY: as the caller promises.
    unsafe {
        let vtbl = *o.cast::<*const usize>();
        core::mem::transmute_copy(&*vtbl.add(slot))
    }
}

pub(crate) type Counted = extern "system" fn(*const c_void) -> u32;

/// Takes a reference of an object a format holds (its collection, its
/// trimming sign): the caller may let go of its own.
pub(crate) fn hold(o: *const c_void) {
    if o.is_null() {
        return;
    }
    // SAFETY: a COM object of the caller's; slot 1 is IUnknown::AddRef.
    let add_ref: Counted = unsafe { method(o, 1) };
    add_ref(o);
}

/// Lets go of a reference a format holds.
pub(crate) fn drop_held(o: *const c_void) {
    if o.is_null() {
        return;
    }
    // SAFETY: a COM object of the caller's; slot 2 is IUnknown::Release.
    let release: Counted = unsafe { method(o, 2) };
    release(o);
}

pub(crate) fn values(this: *const c_void) -> Values {
    // SAFETY: one of our formats.
    unsafe { (*this.cast::<Format>()).values }
}

/// The values of a format, to write one of them through.
///
/// # Safety
/// `this` is one of our formats, and nothing else holds a reference to
/// its values (a COM object is the caller's to keep to one thread).
pub(crate) unsafe fn values_mut(this: *const c_void) -> *mut Values {
    // SAFETY: as the caller promises.
    unsafe { &raw mut (*this.cast_mut().cast::<Format>()).values }
}

/// A format's family name and locale name, each with its length. They
/// are a text layout's own where `this` is one.
pub(crate) fn strings(this: *const c_void) -> ((*const u16, usize), (*const u16, usize)) {
    // SAFETY: one of our formats (or one of our layouts, which begins
    // with the same prefix).
    unsafe {
        let p = this.cast::<Format>();
        ((*p).family, (*p).locale)
    }
}

/// Whether `o` is one of our text formats, which is the only object
/// whose prefix the methods here may read. A layout has the prefix too
/// and answers with a vtable of its own, so this is false for one.
pub(crate) fn ours(o: *const c_void) -> bool {
    if o.is_null() {
        return false;
    }
    // SAFETY: a COM object of the caller's, whose first word is its
    // vtable.
    unsafe { *o.cast::<*const ()>() == vt(&FORMAT_VTBL) }
}

/// Writes a string of a format into the caller's buffer, which must have
/// room for the terminator as well (measured: a buffer of exactly the
/// length is E_NOT_SUFFICIENT_BUFFER).
pub(crate) fn give_string(from: (*const u16, usize), to: *mut u16, size: u32) -> HResult {
    if to.is_null() {
        return E_POINTER;
    }
    let (text, len) = from;
    if (size as usize) <= len {
        return E_NOT_SUFFICIENT_BUFFER;
    }
    // SAFETY: the caller's buffer, which has room for the string and its
    // terminator.
    unsafe { copy_wide(text, to, len) };
    S_OK
}

pub(crate) extern "system" fn format_qi(
    this: *const c_void,
    riid: *const Iid,
    out: *mut *const c_void,
) -> HResult {
    let r = query(this, &[IID_TEXT_FORMAT], riid, out);
    if r == S_OK {
        format_add_ref(this);
    }
    r
}

pub(crate) extern "system" fn format_add_ref(this: *const c_void) -> u32 {
    // SAFETY: one of our formats.
    unsafe { (*this.cast::<Format>()).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

pub(crate) extern "system" fn format_release(this: *const c_void) -> u32 {
    // SAFETY: one of our formats.
    let left = unsafe { (*this.cast::<Format>()).refs.fetch_sub(1, Ordering::AcqRel) } - 1;
    if left == 0 {
        let v = values(this);
        drop_held(v.sign);
        drop_held(v.collection);
        // SAFETY: the last reference is gone; the allocation is `make`'s,
        // whose layout the two lengths give again.
        unsafe {
            let p = this.cast_mut().cast::<Format>();
            let (family_len, locale_len) = ((*p).family.1, (*p).locale.1);
            core::ptr::drop_in_place(p);
            if let Some(layout) = format_alloc(family_len, locale_len) {
                alloc::alloc::dealloc(p.cast(), layout);
            }
        }
    }
    left
}

/// Sets one of the paragraph's enumerated values, refusing anything past
/// the end of its enumeration and leaving the old value behind
/// (measured: a refused setter changes nothing).
fn set_enum(this: *const c_void, value: u32, last: u32, write: fn(*mut Values, u32)) -> HResult {
    if value > last {
        return E_INVALIDARG;
    }
    // SAFETY: one of our formats.
    unsafe { write(values_mut(this), value) };
    S_OK
}

pub(crate) extern "system" fn format_SetTextAlignment(this: *const c_void, value: u32) -> HResult {
    reached!(b"IDWriteTextFormat::SetTextAlignment");
    set_enum(this, value, TEXT_ALIGNMENT_LAST, |v, value| {
        // SAFETY: the values of one of our formats.
        unsafe { (*v).text_alignment = value }
    })
}

pub(crate) extern "system" fn format_SetParagraphAlignment(
    this: *const c_void,
    value: u32,
) -> HResult {
    reached!(b"IDWriteTextFormat::SetParagraphAlignment");
    set_enum(this, value, PARAGRAPH_ALIGNMENT_LAST, |v, value| {
        // SAFETY: the values of one of our formats.
        unsafe { (*v).paragraph_alignment = value }
    })
}

pub(crate) extern "system" fn format_SetWordWrapping(this: *const c_void, value: u32) -> HResult {
    reached!(b"IDWriteTextFormat::SetWordWrapping");
    set_enum(this, value, WORD_WRAPPING_LAST, |v, value| {
        // SAFETY: the values of one of our formats.
        unsafe { (*v).wrapping = value }
    })
}

pub(crate) extern "system" fn format_SetReadingDirection(
    this: *const c_void,
    value: u32,
) -> HResult {
    reached!(b"IDWriteTextFormat::SetReadingDirection");
    set_enum(this, value, READING_DIRECTION_LAST, |v, value| {
        // SAFETY: the values of one of our formats.
        unsafe { (*v).reading = value }
    })
}

pub(crate) extern "system" fn format_SetFlowDirection(this: *const c_void, value: u32) -> HResult {
    reached!(b"IDWriteTextFormat::SetFlowDirection");
    set_enum(this, value, FLOW_DIRECTION_LAST, |v, value| {
        // SAFETY: the values of one of our formats.
        unsafe { (*v).flow = value }
    })
}

/// SetIncrementalTabStop(tabStop) (8): a tab stop must be a positive
/// number; a refused one leaves the old stop (measured).
pub(crate) extern "system" fn format_SetIncrementalTabStop(
    this: *const c_void,
    tab_stop: f32,
) -> HResult {
    reached!(b"IDWriteTextFormat::SetIncrementalTabStop");
    if tab_stop.is_nan() || tab_stop <= 0.0 {
        return E_INVALIDARG;
    }
    // SAFETY: one of our formats.
    unsafe { (*values_mut(this)).tab_stop = tab_stop };
    S_OK
}

/// SetTrimming(&trimming, sign) (9): the granularity, the delimiter and
/// how many of it to count, with an inline object to draw where the text
/// was cut. No trimming options at all is E_INVALIDARG (measured); the
/// sign is optional, and a format holds a reference of its own to it.
pub(crate) extern "system" fn format_SetTrimming(
    this: *const c_void,
    trimming: *const [u32; 3],
    sign: *const c_void,
) -> HResult {
    reached!(b"IDWriteTextFormat::SetTrimming");
    if trimming.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's trimming options.
    let options = unsafe { *trimming };
    if options[0] > TRIMMING_LAST {
        return E_INVALIDARG;
    }
    hold(sign);
    let old = values(this).sign;
    // SAFETY: one of our formats.
    unsafe {
        let v = values_mut(this);
        (*v).trimming = options;
        (*v).sign = sign;
    }
    drop_held(old);
    S_OK
}

/// SetLineSpacing(method, lineSpacing, baseline) (10): the spacing must
/// be zero or more, the baseline is whatever it is (measured: -1 is
/// taken), and the method must be one of DWRITE_LINE_SPACING_METHOD's.
pub(crate) extern "system" fn format_SetLineSpacing(
    this: *const c_void,
    spacing_method: u32,
    spacing: f32,
    baseline: f32,
) -> HResult {
    reached!(b"IDWriteTextFormat::SetLineSpacing");
    if spacing_method > LINE_SPACING_LAST || spacing.is_nan() || spacing < 0.0 || baseline.is_nan()
    {
        return E_INVALIDARG;
    }
    // SAFETY: one of our formats.
    unsafe {
        let v = values_mut(this);
        (*v).spacing_method = spacing_method;
        (*v).spacing = spacing;
        (*v).baseline = baseline;
    }
    S_OK
}

pub(crate) extern "system" fn format_GetTextAlignment(this: *const c_void) -> u32 {
    reached!(b"IDWriteTextFormat::GetTextAlignment");
    values(this).text_alignment
}

pub(crate) extern "system" fn format_GetParagraphAlignment(this: *const c_void) -> u32 {
    reached!(b"IDWriteTextFormat::GetParagraphAlignment");
    values(this).paragraph_alignment
}

pub(crate) extern "system" fn format_GetWordWrapping(this: *const c_void) -> u32 {
    reached!(b"IDWriteTextFormat::GetWordWrapping");
    values(this).wrapping
}

pub(crate) extern "system" fn format_GetReadingDirection(this: *const c_void) -> u32 {
    reached!(b"IDWriteTextFormat::GetReadingDirection");
    values(this).reading
}

pub(crate) extern "system" fn format_GetFlowDirection(this: *const c_void) -> u32 {
    reached!(b"IDWriteTextFormat::GetFlowDirection");
    values(this).flow
}

pub(crate) extern "system" fn format_GetIncrementalTabStop(this: *const c_void) -> f32 {
    reached!(b"IDWriteTextFormat::GetIncrementalTabStop");
    values(this).tab_stop
}

/// GetTrimming(&trimming, &sign) (17): the options and a reference to
/// the sign, which is NULL unless one was set.
pub(crate) extern "system" fn format_GetTrimming(
    this: *const c_void,
    trimming: *mut [u32; 3],
    sign: *mut *const c_void,
) -> HResult {
    reached!(b"IDWriteTextFormat::GetTrimming");
    if trimming.is_null() || sign.is_null() {
        return E_POINTER;
    }
    let v = values(this);
    hold(v.sign);
    // SAFETY: the caller's outputs.
    unsafe {
        *trimming = v.trimming;
        *sign = v.sign;
    }
    S_OK
}

pub(crate) extern "system" fn format_GetLineSpacing(
    this: *const c_void,
    spacing_method: *mut u32,
    spacing: *mut f32,
    baseline: *mut f32,
) -> HResult {
    reached!(b"IDWriteTextFormat::GetLineSpacing");
    if spacing_method.is_null() || spacing.is_null() || baseline.is_null() {
        return E_POINTER;
    }
    let v = values(this);
    // SAFETY: the caller's outputs.
    unsafe {
        *spacing_method = v.spacing_method;
        *spacing = v.spacing;
        *baseline = v.baseline;
    }
    S_OK
}

/// GetFontCollection(&collection) (19): the collection the format was
/// made with, or the system's where a NULL one asked for it.
pub(crate) extern "system" fn format_GetFontCollection(
    this: *const c_void,
    out: *mut *const c_void,
) -> HResult {
    reached!(b"IDWriteTextFormat::GetFontCollection");
    if out.is_null() {
        return E_POINTER;
    }
    let held = values(this).collection;
    let collection = if held.is_null() { font::collection() } else { Some(held) };
    match collection {
        Some(c) => {
            hold(c);
            // SAFETY: the caller's output.
            unsafe { *out = c };
            S_OK
        }
        None => {
            // SAFETY: as above.
            unsafe { *out = core::ptr::null() };
            crate::E_FAIL
        }
    }
}

pub(crate) extern "system" fn format_GetFontFamilyNameLength(this: *const c_void) -> u32 {
    reached!(b"IDWriteTextFormat::GetFontFamilyNameLength");
    strings(this).0.1 as u32
}

pub(crate) extern "system" fn format_GetFontFamilyName(
    this: *const c_void,
    name: *mut u16,
    size: u32,
) -> HResult {
    reached!(b"IDWriteTextFormat::GetFontFamilyName");
    give_string(strings(this).0, name, size)
}

pub(crate) extern "system" fn format_GetFontWeight(this: *const c_void) -> u32 {
    reached!(b"IDWriteTextFormat::GetFontWeight");
    values(this).weight
}

pub(crate) extern "system" fn format_GetFontStyle(this: *const c_void) -> u32 {
    reached!(b"IDWriteTextFormat::GetFontStyle");
    values(this).style
}

pub(crate) extern "system" fn format_GetFontStretch(this: *const c_void) -> u32 {
    reached!(b"IDWriteTextFormat::GetFontStretch");
    values(this).stretch
}

pub(crate) extern "system" fn format_GetFontSize(this: *const c_void) -> f32 {
    reached!(b"IDWriteTextFormat::GetFontSize");
    values(this).size
}

pub(crate) extern "system" fn format_GetLocaleNameLength(this: *const c_void) -> u32 {
    reached!(b"IDWriteTextFormat::GetLocaleNameLength");
    strings(this).1.1 as u32
}

pub(crate) extern "system" fn format_GetLocaleName(
    this: *const c_void,
    name: *mut u16,
    size: u32,
) -> HResult {
    reached!(b"IDWriteTextFormat::GetLocaleName");
    give_string(strings(this).1, name, size)
}

/// The later revisions' methods: refused, each recording its own name
/// (`not_impl!` in `lib.rs` is for the methods whose shape is an
/// HRESULT and no more; these take their arguments and the getters
/// answer a value, so they are written out).
macro_rules! revision_not_impl {
    ($($name:ident($($arg:ty),*) -> $ret:ty as $shown:literal = $refused:expr;)*) => {
        $(
            extern "system" fn $name(_this: *const c_void $(, _: $arg)*) -> $ret {
                crate::missing(concat!("dwrite.dll!", $shown).as_bytes(), 0);
                $refused
            }
        )*
    };
}

pub(crate) use revision_not_impl;

revision_not_impl! {
    format1_SetVerticalGlyphOrientation(u32) -> HResult
        as "IDWriteTextFormat1::SetVerticalGlyphOrientation" = crate::E_NOTIMPL;
    format1_GetVerticalGlyphOrientation() -> u32
        as "IDWriteTextFormat1::GetVerticalGlyphOrientation" = 0;
    format1_SetLastLineWrapping(i32) -> HResult
        as "IDWriteTextFormat1::SetLastLineWrapping" = crate::E_NOTIMPL;
    format1_GetLastLineWrapping() -> i32
        as "IDWriteTextFormat1::GetLastLineWrapping" = 0;
    format1_SetOpticalAlignment(u32) -> HResult
        as "IDWriteTextFormat1::SetOpticalAlignment" = crate::E_NOTIMPL;
    format1_GetOpticalAlignment() -> u32
        as "IDWriteTextFormat1::GetOpticalAlignment" = 0;
    format1_SetFontFallback(*const c_void) -> HResult
        as "IDWriteTextFormat1::SetFontFallback" = crate::E_NOTIMPL;
    format1_GetFontFallback(*mut *const c_void) -> HResult
        as "IDWriteTextFormat1::GetFontFallback" = crate::E_NOTIMPL;
    format2_SetLineSpacing(*const c_void) -> HResult
        as "IDWriteTextFormat2::SetLineSpacing" = crate::E_NOTIMPL;
    format2_GetLineSpacing(*mut c_void) -> HResult
        as "IDWriteTextFormat2::GetLineSpacing" = crate::E_NOTIMPL;
    format3_SetFontAxisValues(*const c_void, u32) -> HResult
        as "IDWriteTextFormat3::SetFontAxisValues" = crate::E_NOTIMPL;
    format3_GetFontAxisValueCount() -> u32
        as "IDWriteTextFormat3::GetFontAxisValueCount" = 0;
    format3_GetFontAxisValues(*mut c_void, u32) -> HResult
        as "IDWriteTextFormat3::GetFontAxisValues" = crate::E_NOTIMPL;
    format3_GetAutomaticFontAxes() -> u32
        as "IDWriteTextFormat3::GetAutomaticFontAxes" = 0;
    format3_SetAutomaticFontAxes(u32) -> HResult
        as "IDWriteTextFormat3::SetAutomaticFontAxes" = crate::E_NOTIMPL;
}

// ---- IDWriteTypography ----------------------------------------------------

/// A set of font features, added one at a time and read back in the
/// order they went in. A tag is four bytes (DWRITE_FONT_FEATURE_TAG) and
/// is not looked at: an unknown feature is the shaper's business.
/// DWRITE_FONT_FEATURE: a feature's four-byte tag
/// (DWRITE_FONT_FEATURE_TAG) and its parameter, passed by value as the
/// eight bytes they are.
#[repr(C)]
#[derive(Clone, Copy, PartialEq)]
pub struct FontFeature {
    tag: u32,
    parameter: u32,
}

#[repr(C)]
struct Typography {
    vtbl: *const (),
    refs: AtomicU32,
    features: Vec<FontFeature>,
}

static TYPOGRAPHY_VTBL: VTable<6> = VTable([
    typography_qi as *const (),
    typography_add_ref as *const (),
    typography_release as *const (),
    typography_AddFontFeature as *const (),
    typography_GetFontFeatureCount as *const (),
    typography_GetFontFeature as *const (),
]);

/// IDWriteFactory::CreateTypography(&typography) (16): an empty set.
pub extern "system" fn factory_CreateTypography(
    _this: *const c_void,
    out: *mut *const c_void,
) -> HResult {
    let r = make_typography(out);
    answered(b"IDWriteFactory::CreateTypography", r);
    r
}

fn make_typography(out: *mut *const c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's output.
    unsafe { *out = core::ptr::null() };
    let layout = core::alloc::Layout::new::<Typography>();
    // SAFETY: a non-zero-sized layout.
    let p = unsafe { alloc::alloc::alloc(layout) }.cast::<Typography>();
    if p.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: fresh memory for one Typography.
    unsafe {
        p.write(Typography {
            vtbl: vt(&TYPOGRAPHY_VTBL),
            refs: AtomicU32::new(1),
            features: Vec::new(),
        });
        *out = p.cast();
    }
    S_OK
}

extern "system" fn typography_qi(
    this: *const c_void,
    riid: *const Iid,
    out: *mut *const c_void,
) -> HResult {
    let r = query(this, &[IID_TYPOGRAPHY], riid, out);
    if r == S_OK {
        typography_add_ref(this);
    }
    r
}

extern "system" fn typography_add_ref(this: *const c_void) -> u32 {
    // SAFETY: one of our typography objects.
    unsafe { (*this.cast::<Typography>()).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

extern "system" fn typography_release(this: *const c_void) -> u32 {
    // SAFETY: one of our typography objects.
    let left = unsafe { (*this.cast::<Typography>()).refs.fetch_sub(1, Ordering::AcqRel) } - 1;
    if left == 0 {
        // SAFETY: the last reference is gone; the allocation is
        // `make_typography`'s, and the drop frees the feature list.
        unsafe {
            let p = this.cast_mut().cast::<Typography>();
            core::ptr::drop_in_place(p);
            alloc::alloc::dealloc(p.cast(), core::alloc::Layout::new::<Typography>());
        }
    }
    left
}

/// AddFontFeature(feature) (3): the feature goes on the end. The list
/// grows fallibly -- a program that cannot have its feature is told so
/// rather than losing the process.
extern "system" fn typography_AddFontFeature(this: *const c_void, feature: FontFeature) -> HResult {
    reached!(b"IDWriteTypography::AddFontFeature");
    // SAFETY: one of our typography objects.
    let features = unsafe { &raw mut (*this.cast_mut().cast::<Typography>()).features };
    // SAFETY: as above; a COM object is the caller's to keep to one thread.
    unsafe {
        if (*features).try_reserve(1).is_err() {
            return E_OUTOFMEMORY;
        }
        (*features).push(feature);
    }
    S_OK
}

extern "system" fn typography_GetFontFeatureCount(this: *const c_void) -> u32 {
    reached!(b"IDWriteTypography::GetFontFeatureCount");
    // SAFETY: one of our typography objects.
    unsafe { (*this.cast::<Typography>()).features.len() as u32 }
}

/// GetFontFeature(index, &feature) (5): the feature at that index, or
/// E_INVALIDARG past the end.
extern "system" fn typography_GetFontFeature(
    this: *const c_void,
    index: u32,
    out: *mut FontFeature,
) -> HResult {
    reached!(b"IDWriteTypography::GetFontFeature");
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: one of our typography objects.
    let features = unsafe { &(*this.cast::<Typography>()).features };
    match features.get(index as usize) {
        Some(&feature) => {
            // SAFETY: the caller's output.
            unsafe { *out = feature };
            S_OK
        }
        None => E_INVALIDARG,
    }
}

// ---- IDWriteNumberSubstitution -------------------------------------------

/// Which digits a locale's numbers are written in: an opaque object (its
/// interface has no methods at all), which a text layout is given and
/// reads through DirectWrite's own means. The values it was made with
/// are kept for the layout that will substitute with them.
#[repr(C)]
struct Substitution {
    vtbl: *const (),
    refs: AtomicU32,
    /// DWRITE_NUMBER_SUBSTITUTION_METHOD, and whether the user's own
    /// digit-substitution setting is ignored. Kept for the text layout
    /// that will substitute with them, which is a chunk of its own and
    /// not written yet -- so nothing reads these two fields today.
    #[allow(dead_code)]
    method: u32,
    #[allow(dead_code)]
    ignore_user_override: i32,
}

static SUBSTITUTION_VTBL: VTable<3> = VTable([
    substitution_qi as *const (),
    substitution_add_ref as *const (),
    substitution_release as *const (),
]);

/// IDWriteFactory::CreateNumberSubstitution(method, localeName,
/// ignoreUserOverride, &substitution) (22). The locale is not looked at
/// at all (measured: "zz-not-a-locale" is taken), but only NONE may be
/// given no locale: the others have a locale's digits to substitute
/// from, and without one they are E_INVALIDARG.
pub extern "system" fn factory_CreateNumberSubstitution(
    _this: *const c_void,
    method: u32,
    locale: *const u16,
    ignore_user_override: i32,
    out: *mut *const c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's output.
    unsafe { *out = core::ptr::null() };
    if method > SUBSTITUTION_LAST || (locale.is_null() && method != SUBSTITUTION_NONE) {
        answered(b"IDWriteFactory::CreateNumberSubstitution", E_INVALIDARG);
        return E_INVALIDARG;
    }
    let layout = core::alloc::Layout::new::<Substitution>();
    // SAFETY: a non-zero-sized layout.
    let p = unsafe { alloc::alloc::alloc(layout) }.cast::<Substitution>();
    if p.is_null() {
        answered(b"IDWriteFactory::CreateNumberSubstitution", E_OUTOFMEMORY);
        return E_OUTOFMEMORY;
    }
    // SAFETY: fresh memory for one Substitution; then the caller's output.
    unsafe {
        p.write(Substitution {
            vtbl: vt(&SUBSTITUTION_VTBL),
            refs: AtomicU32::new(1),
            method,
            ignore_user_override,
        });
        *out = p.cast();
    }
    answered(b"IDWriteFactory::CreateNumberSubstitution", S_OK);
    S_OK
}

extern "system" fn substitution_qi(
    this: *const c_void,
    riid: *const Iid,
    out: *mut *const c_void,
) -> HResult {
    let r = query(this, &[IID_NUMBER_SUBSTITUTION], riid, out);
    if r == S_OK {
        substitution_add_ref(this);
    }
    r
}

extern "system" fn substitution_add_ref(this: *const c_void) -> u32 {
    // SAFETY: one of our substitution objects.
    unsafe { (*this.cast::<Substitution>()).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

extern "system" fn substitution_release(this: *const c_void) -> u32 {
    // SAFETY: one of our substitution objects.
    let left = unsafe { (*this.cast::<Substitution>()).refs.fetch_sub(1, Ordering::AcqRel) } - 1;
    if left == 0 {
        // SAFETY: the last reference is gone; the allocation is this
        // call's own above.
        unsafe {
            let p = this.cast_mut().cast::<Substitution>();
            core::ptr::drop_in_place(p);
            alloc::alloc::dealloc(p.cast(), core::alloc::Layout::new::<Substitution>());
        }
    }
    left
}
