//! DirectWrite's system font collection, as Chromium's browser and Skia
//! use it (ROADMAP BR), with results any Windows install gives (Arial is
//! always there):
//!
//! 1. DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED, IDWriteFactory);
//! 2. GetSystemFontCollection: at least one family; FindFamilyName("Arial")
//!    exists, a made-up name does not;
//! 3. the family's names (en-us) and its regular (400) font: weight 400,
//!    normal stretch and style, HasCharacter('A');
//! 4. the font's face: TrueType, one file, metrics (units per em, ascent,
//!    descent), a glyph for 'A' with a positive advance, a 'cmap' table;
//! 5. the face's file: its loader is a local file loader whose path for
//!    the file's key ends in ".ttf" (the file the font came from), and a
//!    stream over it as long as the file;
//! 6. GetFontFromFontFace gives a font of the same weight;
//! 7. a custom font collection, as Chromium's renderer builds one (its
//!    font proxy): the test's own collection loader enumerates one file,
//!    a custom file reference (CreateCustomFontFileReference) over the
//!    test's own file loader, whose stream serves step 5's file from
//!    memory. CreateCustomFontCollection gives one family; its first
//!    font's face has as many glyphs as the system's face of that file;
//! 8. glyph run analysis (CreateGlyphRunAnalysis) of 'A' at 32 px: in
//!    NATURAL rendering mode the ClearType 3x1 texture has bounds (the
//!    aliased texture's are empty), at most 32 px tall, above the
//!    baseline; CreateAlphaTexture fills some coverage; ALIASED mode gives
//!    an aliased texture (0 or 255) and an empty ClearType one; OUTLINE
//!    mode is E_INVALIDARG;
//! 9. the face's GetGlyphRunOutline for 'A' (em 32) drives a geometry
//!    sink: closed figures with lines or Béziers, above the baseline
//!    (y < 0); the sink is not closed (its owner closes it, as Direct2D's
//!    samples do after GetGlyphRunOutline);
//! 10. text-rendering parameters (CreateRenderingParams, as Direct2D's
//!     drawing state blocks carry them): the factory's own set and a
//!     monitor's hold values in range with the default rendering mode and
//!     grid fit mode; a custom set holds exactly what it was made with;
//!     a gamma outside (0, 256], a negative contrast or ClearType level,
//!     and a pixel geometry or rendering mode past the enumeration are
//!     E_INVALIDARG; one object answers IDWriteRenderingParams 1, 2 and
//!     3, and each call gives a new object;
//! 11. the factory's later revisions and its GDI interop object, as
//!     Word's startup asks for them. GetGdiInterop gives one object per
//!     factory -- the same pointer on a second call -- which answers
//!     IDWriteGdiInterop and IDWriteGdiInterop1 as itself, as Windows'
//!     does. The revisions are the one divergence gala keeps on
//!     purpose: Windows answers IDWriteFactory1 through 7 with this same
//!     factory object, through DWriteCreateFactory and QueryInterface
//!     alike, and gala refuses each with E_NOINTERFACE until the
//!     methods that revision adds exist, because a program asks for the
//!     revision it would like and takes a fallback when it is refused
//!     (Chromium, measured: with IDWriteFactory2 answered, its first
//!     call to it ends the browser on a CHECK). The refusal records the
//!     whole IID, so a run says which revision was wanted. This
//!     program runs on Windows too, so it takes either answer here and
//!     the `dwrite_test` scenario pins gala's;
//! 12. text analysis, which is where Word's startup stops without it:
//!     CreateTextAnalyzer gives an object that answers
//!     IDWriteTextAnalyzer, 1 and 2 as itself (one pointer, three
//!     revisions -- there is no revision 3) and refuses anything else
//!     with E_NOINTERFACE and a NULL slot. Windows makes a fresh
//!     analyzer per call and answers IUnknown with a different pointer
//!     again; gala has one static analyzer and answers IUnknown with
//!     itself, which no caller can tell apart because nothing here is
//!     ever freed, so this step insists only on what both must do.
//!     AnalyzeScript, AnalyzeBidi, AnalyzeNumberSubstitution and
//!     AnalyzeLineBreakpoints read the source through
//!     IDWriteTextAnalysisSource and report through
//!     IDWriteTextAnalysisSink; on "Hello world 123" that is one Latin
//!     script run (script 49, shapes DEFAULT), one bidi run at level 0,
//!     no number substitution runs at all (a silent success: the sink is
//!     never called) and one breakpoint call covering every character,
//!     where a letter may not break on either side (2), each space may
//!     break after it and carries isWhitespace, and the character after
//!     a space may break before it (1). The source is barely consulted:
//!     GetTextAtPosition once at position 0, and GetTextBeforePosition,
//!     GetParagraphReadingDirection, GetLocaleName and
//!     GetNumberSubstitution not at all. GetGlyphs maps those 15
//!     characters to 15 glyphs with the identity cluster map, every one
//!     a cluster start, the two spaces the same glyph and the only one
//!     whose justification is 4 where every letter's is 2;
//!     GetGlyphPlacements scales each glyph's own design advance by the
//!     em size (16 here) and offsets every glyph by nothing. A
//!     zero-length range is S_OK with no runs at all; a NULL sink or
//!     source is E_INVALIDARG. Two edges are measured and not asserted,
//!     because a test cannot assert a crash: real DirectWrite faults on
//!     CreateTextAnalyzer(NULL), writing through the out pointer
//!     unchecked, and faults again when asked to analyze a position past
//!     the end of the source.
//! 13. the text format, which is what Word asks for 279 times a tenth of
//!     a second before its window: CreateTextFormat takes a family name,
//!     a collection (NULL meaning the system's), a weight, a style, a
//!     stretch, an em size and a locale name, and gives an object that
//!     keeps every one of them. It is not a lookup: a family no
//!     collection has, an empty name and a thousand characters are all
//!     taken and read back verbatim, and only NULL is refused. The
//!     weight runs 1..=999 (a value between the enumeration's, 450, is
//!     kept as it is), the style 0..=2, the stretch 1..=9, and the size
//!     must be a positive number -- 0, a negative and a NaN are
//!     E_INVALIDARG where 0.0001 and 1e30 are not. A locale name is
//!     checked for BCP-47's shape and not its length: hyphen-separated
//!     runs of one to eight characters, so two runs of eight are taken
//!     where one run of nine is refused, as are a leading or trailing
//!     hyphen and two running; what is inside a run is never looked at
//!     ("zz-not-a-locale" and "en_us" are both taken). The format starts
//!     leading-aligned both ways, left-to-right and top-to-bottom, with
//!     no wrapping option, the default line spacing, no trimming, and an
//!     incremental tab stop of four times the em size (measured at 16,
//!     10, 7.5 and 0.5). Each setter takes its enumeration's values and
//!     refuses the first past it, leaving the old value behind; the tab
//!     stop must be positive; a uniform line spacing takes a spacing of
//!     zero or more with any baseline at all; trimming takes a
//!     granularity, a delimiter and a count, and no options at all is
//!     E_INVALIDARG. Every call makes a new object, AddRef and Release
//!     count it, and it answers its own interface with itself --
//!     IDWriteTextFormat1, 2 and 3 are the divergence step 11 describes,
//!     so this step takes either answer and insists only that the slot
//!     was written. Typography (CreateTypography) is an empty set of font
//!     features that takes them one at a time, by tag and parameter, and
//!     reads them back in the order they went in, an index past the end
//!     being E_INVALIDARG; a tag is four bytes and is not looked at.
//!     Number substitution (CreateNumberSubstitution) takes a method
//!     0..=4 and does not look at the locale either, except that only
//!     NONE may be given no locale at all -- the others have nothing to
//!     substitute from. And the GDI interop object converts a font or a
//!     face to a LOGFONT, which is how a program that draws through GDI
//!     asks for the font it holds: a LOGFONT of a font and not of a size,
//!     so the height, the width, the escapement and the orientation are
//!     zero, the weight is the font's own, the character set is
//!     DEFAULT_CHARSET and the precision OUT_OUTLINE_PRECIS, and the face
//!     name is the family's own name (measured for two families), which
//!     this step therefore reads from the collection rather than writing
//!     down -- a font of the system collection reports itself a system
//!     font. A NULL font or face is E_INVALIDARG. Measured and not
//!     asserted, a test having no way to assert a crash: real DirectWrite
//!     writes through the out pointers of CreateTextFormat,
//!     CreateTypography, CreateNumberSubstitution, GetTrimming and both
//!     conversions unchecked, and faults on a NULL one.
//! 14. the text layout, the busiest row of Word's whole startup: once
//!     its formats are answered it asks the factory for a
//!     GDI-compatible layout 414 times, and for an ellipsis trimming
//!     sign beside them. A layout is a string, a format and a box, and
//!     it is a text format as well -- it answers the format's own 28
//!     slots from a copy of what it was made with, so what the format is
//!     told afterwards does not reach it. The string's pointer may not
//!     be NULL though its length may be zero, neither side of the box
//!     may be negative though both may be zero, and a GDI-compatible
//!     layout's pixels per DIP are taken as they come -- zero and a
//!     negative are S_OK, as are a transform, a zero transform and
//!     use_gdi_natural. What a layout adds to a format is measurement
//!     and ranges. It measures the text's box inside the layout's
//!     (GetMetrics), the lines it broke the text into (GetLineMetrics),
//!     the unbreakable pieces of it (GetClusterMetrics), the narrowest
//!     box it would fit in (DetermineMinWidth) and how far its ink
//!     reaches past each side (GetOverhangMetrics, positive outside);
//!     each counted getter writes the count and answers
//!     E_NOT_SUFFICIENT_BUFFER when the buffer cannot hold it, except
//!     that no clusters at all is S_OK with a count of zero. Breaking is
//!     greedy at the opportunities the clusters name, keeping a line's
//!     trailing space on it and not counting that space in the width the
//!     text is measured at (the second width is for that); a box too
//!     narrow for one character breaks between characters rather than
//!     overflow. A line break belongs to the line it ends, counting in
//!     its length, its trailing whitespace and its newline length, and a
//!     CRLF counts as the two characters it is -- of which only the LF is
//!     the newline and the place the line may end. Every value a format
//!     holds for the whole text a layout holds over a range of it and
//!     reports with the run it found it in, what no range covers being
//!     the format's own over a run that reaches UINT32_MAX; the values a
//!     format refuses a layout refuses as well, and a drawing effect, an
//!     inline object, a typography and a collection are held as given,
//!     NULL included. Hit testing goes both ways, by point and by
//!     position, and a range of characters is one box of their widths.
//!     The ellipsis trimming sign is an inline object of the format's
//!     font, as wide as the ellipsis it draws and of no height of its
//!     own -- it sits on the baseline it is given -- and it breaks
//!     neither way. And what "GDI-compatible" means is a number, not a
//!     manner: the layout's line height, baseline and width are GDI's
//!     own tmHeight, tmAscent and GetTextExtentPoint32W for the same
//!     family at the same pixel size, which is why this step ships
//!     gdi32.dll and asks it -- measured over four sizes, and at 1.5 and
//!     2 pixels per DIP where an em of e measures what GDI measures at
//!     e * p pixels, reported divided by p. GDI's numbers come from the
//!     face's hinted metrics and are not the design values scaled (Arial
//!     at 24 px reports an ascent of 21 where ascender + lineGap scale to
//!     22.51), so a formula fitted to the design values would hold on
//!     neither system. The plain CreateTextLayout, which Word does not
//!     call at all, is the unrounded one: its line is step 4's design
//!     metrics scaled by the em size, the gap above the ascent belonging
//!     to the baseline. A divergence measured and not asserted: on
//!     Windows a layout asked for IDWriteTextFormat answers an object of
//!     its own where gala answers itself, so only the answer is checked;
//!     its own revisions are step 11's divergence again.

#![no_std]
#![no_main]

use core::ffi::c_void;

type HResult = i32;
type Iid = [u8; 16];

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> *mut c_void;
    fn WriteFile(
        h: *mut c_void,
        buf: *const u8,
        len: u32,
        written: *mut u32,
        ov: *mut c_void,
    ) -> i32;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "dwrite", kind = "raw-dylib")]
unsafe extern "system" {
    fn DWriteCreateFactory(kind: u32, iid: *const Iid, factory: *mut *mut c_void) -> HResult;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    // Step 14's frame holds the structures a layout reports through and
    // is over a page, so the compiler touches each page of it first.
    fn __chkstk();
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

// gdi32, which a GDI-compatible layout is compatible with: the only
// thing its rounded measurements can be compared against (see
// `gdi_text`).
#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateCompatibleDC(dc: *mut c_void) -> *mut c_void;
    fn DeleteDC(dc: *mut c_void) -> i32;
    fn CreateFontIndirectW(lf: *const LogFont) -> *mut c_void;
    fn SelectObject(dc: *mut c_void, o: *mut c_void) -> *mut c_void;
    fn DeleteObject(o: *mut c_void) -> i32;
    fn GetTextMetricsW(dc: *mut c_void, tm: *mut TextMetricW) -> i32;
    fn GetTextExtentPoint32W(dc: *mut c_void, s: *const u16, len: i32, size: *mut [i32; 2]) -> i32;
}

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Iid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

const IID_FACTORY: Iid =
    guid(0xb859ee5a, 0xd838, 0x4b5b, [0xa2, 0xe8, 0x1a, 0xdc, 0x7d, 0x93, 0xdb, 0x48]);
const IID_LOCAL_LOADER: Iid =
    guid(0xb2d9f3ec, 0xc9fe, 0x4a11, [0xa2, 0xec, 0xd8, 0x62, 0x08, 0xf7, 0xc0, 0xa2]);
const IID_UNKNOWN: Iid = guid(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_PARAMS1: Iid =
    guid(0x94413cf4, 0xa6fc, 0x4248, [0x8b, 0x50, 0x66, 0x74, 0x34, 0x8f, 0xca, 0xd3]);
const IID_PARAMS2: Iid =
    guid(0xf9d711c3, 0x9777, 0x40ae, [0x87, 0xe8, 0x3e, 0x5a, 0xf9, 0xbf, 0x09, 0x48]);
const IID_PARAMS3: Iid =
    guid(0xb7924baa, 0x391b, 0x412a, [0x8c, 0x5c, 0xe4, 0x4c, 0xc2, 0xd8, 0x67, 0xdc]);
/// The factory's later revisions, each adding methods to the one before.
const IID_FACTORY_REVISIONS: [Iid; 7] = [
    guid(0x30572f99, 0xdac6, 0x41db, [0xa1, 0x6e, 0x04, 0x86, 0x30, 0x7e, 0x60, 0x6a]),
    guid(0x0439fc60, 0xca44, 0x4994, [0x8d, 0xee, 0x3a, 0x9a, 0xf7, 0xb7, 0x32, 0xec]),
    guid(0x9a1b41c3, 0xd3bb, 0x466a, [0x87, 0xfc, 0xfe, 0x67, 0x55, 0x6a, 0x3b, 0x65]),
    guid(0x4b0b5bd3, 0x0797, 0x4549, [0x8a, 0xc5, 0xfe, 0x91, 0x5c, 0xc5, 0x38, 0x56]),
    guid(0x958db99a, 0xbe2a, 0x4f09, [0xaf, 0x7d, 0x65, 0x18, 0x98, 0x03, 0xd1, 0xd3]),
    guid(0xf3744d80, 0x21f7, 0x42eb, [0xb3, 0x5d, 0x99, 0x5b, 0xc7, 0x2f, 0xc2, 0x23]),
    guid(0x35d0e0b3, 0x9076, 0x4d2e, [0xa0, 0x16, 0xa9, 0x1b, 0x56, 0x8a, 0x06, 0xb4]),
];
const IID_GDI_INTEROP: Iid =
    guid(0x1edd9491, 0x9853, 0x4299, [0x89, 0x8f, 0x64, 0x32, 0x98, 0x3b, 0x6f, 0x3a]);
const IID_GDI_INTEROP1: Iid =
    guid(0x4556be70, 0x3abd, 0x4f70, [0x90, 0xbe, 0x42, 0x17, 0x80, 0xa6, 0xf5, 0x15]);
const IID_ANALYZER: Iid =
    guid(0xb7e6163e, 0x7f46, 0x43b4, [0x84, 0xb3, 0xe4, 0xe6, 0x24, 0x9c, 0x36, 0x5d]);
const IID_ANALYZER1: Iid =
    guid(0x80dad800, 0xe21f, 0x4e83, [0x96, 0xce, 0xbf, 0xcc, 0xe5, 0x00, 0xdb, 0x7c]);
const IID_ANALYZER2: Iid =
    guid(0x553a9ff3, 0x5693, 0x4df7, [0xb5, 0x2b, 0x74, 0x80, 0x6f, 0x7f, 0x2e, 0xb9]);
const IID_ANALYSIS_SOURCE: Iid =
    guid(0x688e1a58, 0x5094, 0x47c8, [0xad, 0xc8, 0xfb, 0xce, 0xa6, 0x0a, 0xe9, 0x2b]);
const IID_ANALYSIS_SINK: Iid =
    guid(0x5810cd44, 0x0ca0, 0x4701, [0xb3, 0xfa, 0xbe, 0xc5, 0x18, 0x2a, 0xe4, 0xf6]);
const IID_TEXT_FORMAT: Iid =
    guid(0x9c906818, 0x31d7, 0x4fd3, [0xa1, 0x51, 0x7c, 0x5e, 0x22, 0x5d, 0xb5, 0x5a]);
/// A text format's later revisions, each adding methods to the one before.
const IID_TEXT_FORMAT_REVISIONS: [Iid; 3] = [
    guid(0x5f174b49, 0x0d8b, 0x4cfb, [0x8b, 0xca, 0xf1, 0xcc, 0xe9, 0xd0, 0x6c, 0x67]),
    guid(0xf67e0edd, 0x9e3d, 0x4ecc, [0x8c, 0x32, 0x41, 0x83, 0x25, 0x3d, 0xfe, 0x70]),
    guid(0x6d3b5641, 0xe550, 0x430d, [0xa8, 0x5b, 0xb7, 0xbf, 0x48, 0xa9, 0x34, 0x27]),
];
const IID_TEXT_LAYOUT: Iid =
    guid(0x53737037, 0x6d14, 0x410b, [0x9b, 0xfe, 0x0b, 0x18, 0x2b, 0xb7, 0x09, 0x61]);
/// A text layout's later revisions, as a text format's are.
const IID_TEXT_LAYOUT_REVISIONS: [Iid; 4] = [
    guid(0x9064d822, 0x80a7, 0x465c, [0xa9, 0x86, 0xdf, 0x65, 0xf7, 0x8b, 0x8f, 0xeb]),
    guid(0x1093c18f, 0x8d5e, 0x43f0, [0xb0, 0x64, 0x09, 0x17, 0x31, 0x1b, 0x52, 0x5e]),
    guid(0x07ddcd52, 0x020e, 0x4de8, [0xac, 0x33, 0x6c, 0x95, 0x3d, 0x83, 0xf9, 0x2d]),
    guid(0x05a9bf42, 0x223f, 0x4441, [0xb5, 0xfb, 0x82, 0x63, 0x68, 0x5f, 0x55, 0xe9]),
];
/// What a layout puts in a line in place of text: the ellipsis trimming
/// sign is one.
const IID_INLINE_OBJECT: Iid =
    guid(0x8339fde3, 0x106f, 0x47ab, [0x83, 0x73, 0x1c, 0x62, 0x95, 0xeb, 0x10, 0xb3]);
/// A GUID of no interface at all, for the refusal every QueryInterface
/// owes (made up here, so nothing will ever answer it).
const IID_NOT_AN_INTERFACE: Iid = guid(0x1b1b1b1b, 0x2c2c, 0x3d3d, [0x4e; 8]);

/// The test's objects' QueryInterface: themselves for IUnknown and their
/// own interface, E_NOINTERFACE (and NULL) for anything else. DirectWrite
/// on Windows asks a custom file loader for its extended interfaces
/// (IDWriteRemoteFontFileLoader) and calls their methods when told yes,
/// past the end of a vtable that has only IDWriteFontFileLoader's.
///
/// # Safety
/// `iid` and `out` are the caller's, as QueryInterface's contract has them.
unsafe fn query(this: *mut c_void, own: &Iid, iid: *const Iid, out: *mut *mut c_void) -> HResult {
    // SAFETY: the caller's IID and output.
    unsafe {
        if *iid == IID_UNKNOWN || *iid == *own {
            *out = this;
            0
        } else {
            *out = core::ptr::null_mut();
            0x8000_4002_u32 as i32 // E_NOINTERFACE
        }
    }
}

fn out(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        )
    };
}

fn fail(n: u32) -> ! {
    let mut line = *b"dwrite_test: check 00 failed\r\n";
    line[19] = b'0' + (n / 10) as u8;
    line[20] = b'0' + (n % 10) as u8;
    out(&line);
    // SAFETY: ends the process.
    unsafe { ExitProcess(n) }
}

fn check(ok: bool, n: u32) {
    if !ok {
        fail(n);
    }
}

/// Method `slot` of COM object `this`, as a function of type `F`.
///
/// # Safety
/// `this` is a COM object whose vtable has that slot with that type.
unsafe fn method<F: Copy>(this: *mut c_void, slot: usize) -> F {
    // SAFETY: as the caller promises.
    unsafe {
        let vtbl = *this.cast::<*const usize>();
        core::mem::transmute_copy(&*vtbl.add(slot))
    }
}

type GetPtr = extern "system" fn(*mut c_void, *mut *mut c_void) -> HResult;
type GetIndexed = extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HResult;
type GetU32 = extern "system" fn(*mut c_void) -> u32;
type GetPtr1 = extern "system" fn(*mut c_void, *mut c_void) -> HResult;
type GetF32 = extern "system" fn(*mut c_void) -> f32;

/// IDWriteFactory::CreateCustomRenderingParams (slot 12).
type MakeCustom =
    extern "system" fn(*mut c_void, f32, f32, f32, u32, u32, *mut *mut c_void) -> HResult;

/// The seven values a parameter set holds, read through the four
/// interfaces one object answers (dwrite.h, dwrite_1.h, dwrite_2.h,
/// dwrite_3.h): gamma, enhanced contrast, ClearType level, pixel
/// geometry, rendering mode, grayscale enhanced contrast, grid fit mode
/// and the rendering mode ...3 reports.
///
/// # Safety
/// `params` is an IDWriteRenderingParams of this DirectWrite.
unsafe fn values_of(params: *mut c_void) -> (f32, f32, f32, u32, u32, f32, u32, u32) {
    // SAFETY: as the caller promises; the slots are the headers'.
    unsafe {
        let gamma: GetF32 = method(params, 3);
        let contrast: GetF32 = method(params, 4);
        let cleartype: GetF32 = method(params, 5);
        let geometry: GetU32 = method(params, 6);
        let mode: GetU32 = method(params, 7);
        let grayscale: GetF32 = method(params, 8);
        let grid_fit: GetU32 = method(params, 9);
        let mode1: GetU32 = method(params, 10);
        (
            gamma(params),
            contrast(params),
            cleartype(params),
            geometry(params),
            mode(params),
            grayscale(params),
            grid_fit(params),
            mode1(params),
        )
    }
}

/// Step 8's texture buffer (static: a frame over a page would need a
/// stack probe).
static mut ALPHA: [u8; 3 * 48 * 48] = [0; 3 * 48 * 48];

/// Step 13's thousand-character family name, static for the same reason.
static mut LONG_NAME: [u16; 1024] = [0; 1024];

/// Step 9's ID2D1SimplifiedGeometrySink: counts what it is told.
mod sink {
    use super::{HResult, Iid, Obj, VTable};
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    pub static FIGURES: AtomicU32 = AtomicU32::new(0);
    pub static ENDED: AtomicU32 = AtomicU32::new(0);
    pub static SEGMENTS: AtomicU32 = AtomicU32::new(0);
    pub static CLOSED: AtomicU32 = AtomicU32::new(0);
    /// Some point is above the baseline (y < 0: y grows downward).
    pub static ABOVE: AtomicBool = AtomicBool::new(false);

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Point {
        x: f32,
        y: f32,
    }

    static VTBL: VTable<10> = VTable([
        qi as *const (),
        add_ref as *const (),
        release as *const (),
        set_fill_mode as *const (),
        set_segment_flags as *const (),
        begin_figure as *const (),
        add_lines as *const (),
        add_beziers as *const (),
        end_figure as *const (),
        close as *const (),
    ]);
    static SINK: Obj = Obj(VTBL.0.as_ptr().cast());

    pub fn sink() -> *mut c_void {
        (&raw const SINK).cast_mut().cast()
    }

    fn see(p: Point) {
        if p.y < -1.0 {
            ABOVE.store(true, Ordering::Relaxed);
        }
    }

    /// ID2D1SimplifiedGeometrySink (IDWriteGeometrySink is the same).
    const IID_SINK: Iid =
        super::guid(0x2cd9069e, 0x12e2, 0x11dc, [0x9f, 0xed, 0x00, 0x11, 0x43, 0xa0, 0x55, 0xf9]);
    extern "system" fn qi(this: *mut c_void, iid: *const Iid, out: *mut *mut c_void) -> HResult {
        // SAFETY: QueryInterface's arguments.
        unsafe { super::query(this, &IID_SINK, iid, out) }
    }
    extern "system" fn add_ref(_: *mut c_void) -> u32 {
        1
    }
    extern "system" fn release(_: *mut c_void) -> u32 {
        1
    }
    extern "system" fn set_fill_mode(_: *mut c_void, _: u32) {}
    extern "system" fn set_segment_flags(_: *mut c_void, _: u32) {}
    extern "system" fn begin_figure(_: *mut c_void, start: Point, _: u32) {
        FIGURES.fetch_add(1, Ordering::Relaxed);
        see(start);
    }
    extern "system" fn add_lines(_: *mut c_void, points: *const Point, count: u32) {
        SEGMENTS.fetch_add(count, Ordering::Relaxed);
        for i in 0..count as usize {
            // SAFETY: the caller's `count` points.
            see(unsafe { *points.add(i) });
        }
    }
    extern "system" fn add_beziers(_: *mut c_void, beziers: *const [Point; 3], count: u32) {
        SEGMENTS.fetch_add(count, Ordering::Relaxed);
        for i in 0..count as usize {
            // SAFETY: the caller's `count` segments.
            see(unsafe { (*beziers.add(i))[2] });
        }
    }
    extern "system" fn end_figure(_: *mut c_void, _: u32) {
        ENDED.fetch_add(1, Ordering::Relaxed);
    }
    extern "system" fn close(_: *mut c_void) -> HResult {
        CLOSED.fetch_add(1, Ordering::Relaxed);
        0
    }
}

/// Step 7's objects: a collection loader whose enumerator gives one custom
/// file reference (key: the number 1) over a file loader whose stream
/// serves `DATA` (the font file's bytes). Static objects: AddRef and
/// Release count nothing.
mod custom {
    use super::{HResult, Iid, Obj, VTable};
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicPtr, AtomicU32, AtomicUsize, Ordering};

    pub static DATA: AtomicPtr<u8> = AtomicPtr::new(core::ptr::null_mut());
    pub static LEN: AtomicUsize = AtomicUsize::new(0);
    pub static FACTORY: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
    /// The enumerator's position: 0 before the file, 1 on it, 2 past it.
    static POSITION: AtomicU32 = AtomicU32::new(0);
    static FILE_KEY: u32 = 1;

    static FILE_LOADER_VTBL: VTable<4> = VTable([
        qi as *const (),
        add_ref as *const (),
        release as *const (),
        create_stream as *const (),
    ]);
    static STREAM_VTBL: VTable<7> = VTable([
        qi as *const (),
        add_ref as *const (),
        release as *const (),
        read_fragment as *const (),
        release_fragment as *const (),
        file_size as *const (),
        write_time as *const (),
    ]);
    static COLLECTION_LOADER_VTBL: VTable<4> = VTable([
        qi as *const (),
        add_ref as *const (),
        release as *const (),
        create_enumerator as *const (),
    ]);
    static ENUMERATOR_VTBL: VTable<5> = VTable([
        qi as *const (),
        add_ref as *const (),
        release as *const (),
        move_next as *const (),
        current as *const (),
    ]);

    static FILE_LOADER: Obj = Obj(FILE_LOADER_VTBL.0.as_ptr().cast());
    static STREAM: Obj = Obj(STREAM_VTBL.0.as_ptr().cast());
    static COLLECTION_LOADER: Obj = Obj(COLLECTION_LOADER_VTBL.0.as_ptr().cast());
    static ENUMERATOR: Obj = Obj(ENUMERATOR_VTBL.0.as_ptr().cast());

    pub fn file_loader() -> *mut c_void {
        (&raw const FILE_LOADER).cast_mut().cast()
    }
    pub fn collection_loader() -> *mut c_void {
        (&raw const COLLECTION_LOADER).cast_mut().cast()
    }

    const IID_FILE_LOADER: Iid =
        super::guid(0x727cad4e, 0xd6af, 0x4c9e, [0x8a, 0x08, 0xd6, 0x95, 0xb1, 0x1c, 0xaa, 0x49]);
    const IID_STREAM: Iid =
        super::guid(0x6d4865fe, 0x0ab8, 0x4d91, [0x8f, 0x62, 0x5d, 0xd6, 0xbe, 0x34, 0xa3, 0xe0]);
    const IID_COLLECTION_LOADER: Iid =
        super::guid(0xcca920e4, 0x52f0, 0x492b, [0xbf, 0xa8, 0x29, 0xc7, 0x2e, 0xe0, 0xa4, 0x68]);
    const IID_ENUMERATOR: Iid =
        super::guid(0x72755049, 0x5ff7, 0x435d, [0x83, 0x48, 0x4b, 0xe9, 0x7c, 0xfa, 0x6c, 0x7c]);
    /// Each object's own interface; QueryInterface answers it and IUnknown.
    extern "system" fn qi(this: *mut c_void, iid: *const Iid, out: *mut *mut c_void) -> HResult {
        let own = if this == (&raw const FILE_LOADER).cast_mut().cast() {
            &IID_FILE_LOADER
        } else if this == (&raw const STREAM).cast_mut().cast() {
            &IID_STREAM
        } else if this == (&raw const COLLECTION_LOADER).cast_mut().cast() {
            &IID_COLLECTION_LOADER
        } else {
            &IID_ENUMERATOR
        };
        // SAFETY: QueryInterface's arguments.
        unsafe { super::query(this, own, iid, out) }
    }
    extern "system" fn add_ref(_: *mut c_void) -> u32 {
        1
    }
    extern "system" fn release(_: *mut c_void) -> u32 {
        1
    }
    extern "system" fn create_stream(
        _: *mut c_void,
        key: *const u32,
        size: u32,
        out: *mut *mut c_void,
    ) -> HResult {
        // SAFETY: the reference key handed out below.
        if size != 4 || unsafe { *key } != FILE_KEY {
            return 0x8000_4005_u32 as i32; // E_FAIL
        }
        // SAFETY: the caller's output.
        unsafe { *out = (&raw const STREAM).cast_mut().cast() };
        0
    }
    extern "system" fn read_fragment(
        _: *mut c_void,
        start: *mut *const u8,
        offset: u64,
        size: u64,
        context: *mut *mut c_void,
    ) -> HResult {
        let len = LEN.load(Ordering::Relaxed) as u64;
        if offset.checked_add(size).is_none_or(|end| end > len) {
            return 0x8000_4005_u32 as i32;
        }
        // SAFETY: inside the file's bytes; the caller's outputs.
        unsafe {
            *start = DATA.load(Ordering::Relaxed).add(offset as usize);
            *context = core::ptr::null_mut();
        }
        0
    }
    extern "system" fn release_fragment(_: *mut c_void, _: *mut c_void) {}
    extern "system" fn file_size(_: *mut c_void, size: *mut u64) -> HResult {
        // SAFETY: the caller's output.
        unsafe { *size = LEN.load(Ordering::Relaxed) as u64 };
        0
    }
    extern "system" fn write_time(_: *mut c_void, time: *mut u64) -> HResult {
        // SAFETY: the caller's output.
        unsafe { *time = 0 };
        0
    }
    extern "system" fn create_enumerator(
        _: *mut c_void,
        _factory: *mut c_void,
        key: *const u32,
        size: u32,
        out: *mut *mut c_void,
    ) -> HResult {
        // SAFETY: the test's collection key.
        if size != 4 || unsafe { *key } != 7 {
            return 0x8007_0057_u32 as i32; // E_INVALIDARG
        }
        POSITION.store(0, Ordering::Relaxed);
        // SAFETY: the caller's output.
        unsafe { *out = (&raw const ENUMERATOR).cast_mut().cast() };
        0
    }
    extern "system" fn move_next(_: *mut c_void, has: *mut i32) -> HResult {
        let p = POSITION.fetch_add(1, Ordering::Relaxed) + 1;
        // SAFETY: the caller's output.
        unsafe { *has = i32::from(p == 1) };
        0
    }
    extern "system" fn current(_: *mut c_void, file: *mut *mut c_void) -> HResult {
        if POSITION.load(Ordering::Relaxed) != 1 {
            return 0x8000_4005_u32 as i32;
        }
        let factory = FACTORY.load(Ordering::Relaxed);
        // SAFETY: IDWriteFactory::CreateCustomFontFileReference (slot 8).
        unsafe {
            let create: extern "system" fn(
                *mut c_void,
                *const c_void,
                u32,
                *mut c_void,
                *mut *mut c_void,
            ) -> HResult = super::method(factory, 8);
            create(factory, (&raw const FILE_KEY).cast(), 4, file_loader(), file)
        }
    }
}

/// The COM object every test module hands out: a pointer to its vtable and
/// nothing else, and the vtable itself. One pair for the whole file, so
/// each module describes only its own slots.
#[repr(C)]
struct Obj(*const ());
struct VTable<const N: usize>([*const (); N]);
// SAFETY (both): pointers to static vtables and to functions.
unsafe impl Sync for Obj {}
unsafe impl<const N: usize> Sync for VTable<N> {}

/// DWRITE_SCRIPT_ANALYSIS: a script number and DWRITE_SCRIPT_SHAPES, which
/// is a C enum and so four bytes. Measured on Windows 11 24H2: the whole
/// struct is 8 bytes with `shapes` at offset 4, which two packed u16s would
/// get wrong without a word from the compiler.
#[repr(C)]
struct ScriptAnalysis {
    script: u16,
    shapes: u32,
}

/// The four Analyze* methods share one signature: a source to read, a range
/// in it, and a sink to report to.
type Analyze = extern "system" fn(*mut c_void, *mut c_void, u32, u32, *mut c_void) -> HResult;

/// IDWriteTextAnalyzer::GetGlyphs. The two shaping bitfield structs are one
/// u16 each (measured), so they pass as u16 slots.
#[allow(clippy::type_complexity)]
type GetGlyphs = extern "system" fn(
    *mut c_void, // this
    *const u16,  // text
    u32,         // text length
    *mut c_void, // font face
    i32,         // is sideways
    i32,         // is right-to-left
    *const ScriptAnalysis,
    *const u16,           // locale
    *mut c_void,          // number substitution
    *const *const c_void, // typographic features
    *const u32,           // feature range lengths
    u32,                  // feature ranges
    u32,                  // max glyph count
    *mut u16,             // cluster map
    *mut u16,             // shaping text properties
    *mut u16,             // glyph indices
    *mut u16,             // shaping glyph properties
    *mut u32,             // glyphs written
) -> HResult;

/// IDWriteTextAnalyzer::GetGlyphPlacements. An offset is two floats
/// (advance, ascender), measured 8 bytes with no padding.
#[allow(clippy::type_complexity)]
type GetPlacements = extern "system" fn(
    *mut c_void, // this
    *const u16,  // text
    *const u16,  // cluster map
    *mut u16,    // shaping text properties
    u32,         // text length
    *const u16,  // glyph indices
    *const u16,  // shaping glyph properties
    u32,         // glyph count
    *mut c_void, // font face
    f32,         // em size
    i32,         // is sideways
    i32,         // is right-to-left
    *const ScriptAnalysis,
    *const u16,           // locale
    *const *const c_void, // typographic features
    *const u32,           // feature range lengths
    u32,                  // feature ranges
    *mut f32,             // advances
    *mut [f32; 2],        // offsets
) -> HResult;

/// Step 12's IDWriteTextAnalysisSource and IDWriteTextAnalysisSink: one
/// block of text from position 0, and a count of everything the analyzer
/// asks for and reports. Windows consults the source once (at position 0)
/// and never calls the other four methods at all, so what is counted here
/// is as much of the contract as what comes back through the sink.
mod analysis {
    use super::{
        HResult, IID_ANALYSIS_SINK, IID_ANALYSIS_SOURCE, Iid, Obj, ScriptAnalysis, VTable,
    };
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicPtr, AtomicU8, AtomicU32, Ordering};

    /// The text under analysis. Built at run time because `u16::from` is
    /// not const, exactly as the custom loader's data block is.
    static TEXT: AtomicPtr<u16> = AtomicPtr::new(core::ptr::null_mut());
    static LEN: AtomicU32 = AtomicU32::new(0);

    /// "en-us", NUL terminated.
    static LOCALE: [u16; 6] = [0x65, 0x6e, 0x2d, 0x75, 0x73, 0];

    /// What the source was asked for: the five methods' call counts, and
    /// the highest position GetTextAtPosition was given.
    static AT: AtomicU32 = AtomicU32::new(0);
    static BEFORE: AtomicU32 = AtomicU32::new(0);
    static DIR: AtomicU32 = AtomicU32::new(0);
    static LOC: AtomicU32 = AtomicU32::new(0);
    static SUBST: AtomicU32 = AtomicU32::new(0);
    static AT_MAX: AtomicU32 = AtomicU32::new(0);

    /// What the sink was told. Each run count stands beside the last run's
    /// own numbers, which is enough while the test's strings produce one
    /// run each.
    static SCRIPT_RUNS: AtomicU32 = AtomicU32::new(0);
    static SCRIPT_POS: AtomicU32 = AtomicU32::new(0);
    static SCRIPT_LEN: AtomicU32 = AtomicU32::new(0);
    static SCRIPT_ID: AtomicU32 = AtomicU32::new(0);
    static SCRIPT_SHAPES: AtomicU32 = AtomicU32::new(0);
    static BIDI_RUNS: AtomicU32 = AtomicU32::new(0);
    static BIDI_POS: AtomicU32 = AtomicU32::new(0);
    static BIDI_LEN: AtomicU32 = AtomicU32::new(0);
    static BIDI_EXPLICIT: AtomicU32 = AtomicU32::new(0);
    static BIDI_RESOLVED: AtomicU32 = AtomicU32::new(0);
    static SUBST_RUNS: AtomicU32 = AtomicU32::new(0);
    static BREAK_CALLS: AtomicU32 = AtomicU32::new(0);
    static BREAK_POS: AtomicU32 = AtomicU32::new(0);
    static BREAK_LEN: AtomicU32 = AtomicU32::new(0);
    /// One DWRITE_LINE_BREAKPOINT is a single byte of bitfields (measured).
    static BREAK: [AtomicU8; 16] = [const { AtomicU8::new(0) }; 16];

    /// Point the source at `text` and forget every earlier answer, so each
    /// Analyze* call is measured on its own.
    pub fn arm(text: *const u16, len: u32) {
        TEXT.store(text.cast_mut(), Ordering::Relaxed);
        LEN.store(len, Ordering::Relaxed);
        for c in [
            &AT,
            &BEFORE,
            &DIR,
            &LOC,
            &SUBST,
            &AT_MAX,
            &SCRIPT_RUNS,
            &SCRIPT_POS,
            &SCRIPT_LEN,
            &SCRIPT_ID,
            &SCRIPT_SHAPES,
            &BIDI_RUNS,
            &BIDI_POS,
            &BIDI_LEN,
            &BIDI_EXPLICIT,
            &BIDI_RESOLVED,
            &SUBST_RUNS,
            &BREAK_CALLS,
            &BREAK_POS,
            &BREAK_LEN,
        ] {
            c.store(0, Ordering::Relaxed);
        }
        for b in &BREAK {
            b.store(0, Ordering::Relaxed);
        }
    }

    /// [GetTextAtPosition, GetTextBeforePosition, GetParagraphReadingDirection,
    /// GetLocaleName, GetNumberSubstitution, the highest position asked for].
    pub fn source_calls() -> [u32; 6] {
        [
            AT.load(Ordering::Relaxed),
            BEFORE.load(Ordering::Relaxed),
            DIR.load(Ordering::Relaxed),
            LOC.load(Ordering::Relaxed),
            SUBST.load(Ordering::Relaxed),
            AT_MAX.load(Ordering::Relaxed),
        ]
    }

    /// [runs, the last run's position, its length, its script, its shapes].
    pub fn script() -> [u32; 5] {
        [
            SCRIPT_RUNS.load(Ordering::Relaxed),
            SCRIPT_POS.load(Ordering::Relaxed),
            SCRIPT_LEN.load(Ordering::Relaxed),
            SCRIPT_ID.load(Ordering::Relaxed),
            SCRIPT_SHAPES.load(Ordering::Relaxed),
        ]
    }

    /// [runs, position, length, explicit level, resolved level].
    pub fn bidi() -> [u32; 5] {
        [
            BIDI_RUNS.load(Ordering::Relaxed),
            BIDI_POS.load(Ordering::Relaxed),
            BIDI_LEN.load(Ordering::Relaxed),
            BIDI_EXPLICIT.load(Ordering::Relaxed),
            BIDI_RESOLVED.load(Ordering::Relaxed),
        ]
    }

    pub fn subst_runs() -> u32 {
        SUBST_RUNS.load(Ordering::Relaxed)
    }

    /// [SetLineBreakpoints calls, the last call's position, its length].
    pub fn breaks() -> [u32; 3] {
        [
            BREAK_CALLS.load(Ordering::Relaxed),
            BREAK_POS.load(Ordering::Relaxed),
            BREAK_LEN.load(Ordering::Relaxed),
        ]
    }

    pub fn break_at(i: usize) -> u8 {
        BREAK[i].load(Ordering::Relaxed)
    }

    extern "system" fn src_qi(
        this: *mut c_void,
        iid: *const Iid,
        out: *mut *mut c_void,
    ) -> HResult {
        // SAFETY: the analyzer's IID and output slot.
        unsafe { super::query(this, &IID_ANALYSIS_SOURCE, iid, out) }
    }

    extern "system" fn snk_qi(
        this: *mut c_void,
        iid: *const Iid,
        out: *mut *mut c_void,
    ) -> HResult {
        // SAFETY: the analyzer's IID and output slot.
        unsafe { super::query(this, &IID_ANALYSIS_SINK, iid, out) }
    }

    extern "system" fn add_ref(_this: *mut c_void) -> u32 {
        2
    }

    extern "system" fn release(_this: *mut c_void) -> u32 {
        1
    }

    /// GetTextAtPosition: the one block, from `pos` on. Past the end is
    /// NULL and zero, as a source must answer.
    extern "system" fn src_at(
        _this: *mut c_void,
        pos: u32,
        text: *mut *const u16,
        len: *mut u32,
    ) -> HResult {
        AT.fetch_add(1, Ordering::Relaxed);
        AT_MAX.fetch_max(pos, Ordering::Relaxed);
        let whole = LEN.load(Ordering::Relaxed);
        // SAFETY: the analyzer's two output slots.
        unsafe {
            if pos >= whole {
                *text = core::ptr::null();
                *len = 0;
            } else {
                *text = TEXT.load(Ordering::Relaxed).cast_const().add(pos as usize);
                *len = whole - pos;
            }
        }
        0
    }

    /// GetTextBeforePosition: the block up to `pos`. Windows never calls
    /// this for any of step 12's analyses, which is itself asserted.
    extern "system" fn src_before(
        _this: *mut c_void,
        pos: u32,
        text: *mut *const u16,
        len: *mut u32,
    ) -> HResult {
        BEFORE.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the analyzer's two output slots.
        unsafe {
            if pos == 0 || pos > LEN.load(Ordering::Relaxed) {
                *text = core::ptr::null();
                *len = 0;
            } else {
                *text = TEXT.load(Ordering::Relaxed).cast_const();
                *len = pos;
            }
        }
        0
    }

    /// GetParagraphReadingDirection returns a DWRITE_READING_DIRECTION and
    /// not an HRESULT: 0 is left to right.
    extern "system" fn src_dir(_this: *mut c_void) -> u32 {
        DIR.fetch_add(1, Ordering::Relaxed);
        0
    }

    extern "system" fn src_locale(
        _this: *mut c_void,
        _pos: u32,
        len: *mut u32,
        locale: *mut *const u16,
    ) -> HResult {
        LOC.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the analyzer's two output slots.
        unsafe {
            *len = LEN.load(Ordering::Relaxed);
            *locale = LOCALE.as_ptr();
        }
        0
    }

    extern "system" fn src_subst(
        _this: *mut c_void,
        _pos: u32,
        len: *mut u32,
        subst: *mut *mut c_void,
    ) -> HResult {
        SUBST.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the analyzer's two output slots.
        unsafe {
            *len = LEN.load(Ordering::Relaxed);
            *subst = core::ptr::null_mut();
        }
        0
    }

    extern "system" fn snk_script(
        _this: *mut c_void,
        pos: u32,
        len: u32,
        sa: *const ScriptAnalysis,
    ) -> HResult {
        SCRIPT_RUNS.fetch_add(1, Ordering::Relaxed);
        SCRIPT_POS.store(pos, Ordering::Relaxed);
        SCRIPT_LEN.store(len, Ordering::Relaxed);
        // SAFETY: the analysis the analyzer hands us for the call's duration.
        unsafe {
            SCRIPT_ID.store(u32::from((*sa).script), Ordering::Relaxed);
            SCRIPT_SHAPES.store((*sa).shapes, Ordering::Relaxed);
        }
        0
    }

    extern "system" fn snk_breaks(
        _this: *mut c_void,
        pos: u32,
        len: u32,
        bp: *const u8,
    ) -> HResult {
        BREAK_CALLS.fetch_add(1, Ordering::Relaxed);
        BREAK_POS.store(pos, Ordering::Relaxed);
        BREAK_LEN.store(len, Ordering::Relaxed);
        for i in 0..len as usize {
            if pos as usize + i >= BREAK.len() {
                break;
            }
            // SAFETY: `len` breakpoints, one byte each, for this call.
            let b = unsafe { *bp.add(i) };
            BREAK[pos as usize + i].store(b, Ordering::Relaxed);
        }
        0
    }

    extern "system" fn snk_bidi(
        _this: *mut c_void,
        pos: u32,
        len: u32,
        explicit: u8,
        resolved: u8,
    ) -> HResult {
        BIDI_RUNS.fetch_add(1, Ordering::Relaxed);
        BIDI_POS.store(pos, Ordering::Relaxed);
        BIDI_LEN.store(len, Ordering::Relaxed);
        BIDI_EXPLICIT.store(u32::from(explicit), Ordering::Relaxed);
        BIDI_RESOLVED.store(u32::from(resolved), Ordering::Relaxed);
        0
    }

    extern "system" fn snk_subst(
        _this: *mut c_void,
        _pos: u32,
        _len: u32,
        _subst: *mut c_void,
    ) -> HResult {
        SUBST_RUNS.fetch_add(1, Ordering::Relaxed);
        0
    }

    static SOURCE_VTBL: VTable<8> = VTable([
        src_qi as *const (),
        add_ref as *const (),
        release as *const (),
        src_at as *const (),
        src_before as *const (),
        src_dir as *const (),
        src_locale as *const (),
        src_subst as *const (),
    ]);

    static SINK_VTBL: VTable<7> = VTable([
        snk_qi as *const (),
        add_ref as *const (),
        release as *const (),
        snk_script as *const (),
        snk_breaks as *const (),
        snk_bidi as *const (),
        snk_subst as *const (),
    ]);

    static SOURCE: Obj = Obj(SOURCE_VTBL.0.as_ptr().cast());
    static SINK: Obj = Obj(SINK_VTBL.0.as_ptr().cast());

    pub fn source() -> *mut c_void {
        (&raw const SOURCE).cast_mut().cast()
    }

    pub fn sink() -> *mut c_void {
        (&raw const SINK).cast_mut().cast()
    }
}

fn w(s: &str, buf: &mut [u16; 64]) -> *const u16 {
    for (i, c) in s.bytes().enumerate() {
        buf[i] = u16::from(c);
    }
    buf[s.len()] = 0;
    buf.as_ptr()
}

/// LOGFONTW (wingdi.h), which the GDI interop object fills in.
#[repr(C)]
#[derive(Clone, Copy)]
struct LogFont {
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
    face_name: [u16; 32],
}

impl LogFont {
    /// Every field zero, so a conversion that writes nothing is seen to.
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
        face_name: [0; 32],
    };
}

/// The 92 bytes GDI's structure is: what the conversions write through.
const _: () = assert!(core::mem::size_of::<LogFont>() == 92);

/// The characters of `s` before its terminator.
fn chars(s: &[u16]) -> &[u16] {
    &s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())]
}

/// Whether `s` holds `want` and then a terminator.
fn is_text(s: &[u16], want: &str) -> bool {
    chars(s).len() == want.len() && want.bytes().zip(s).all(|(b, &c)| u16::from(b) == c)
}

/// Releases a COM object (Release is slot 2 of every interface).
///
/// # Safety
/// `o` is a COM object this call owns a reference of.
unsafe fn drop_com(o: *mut c_void) {
    // SAFETY: as the caller promises.
    let release: GetU32 = unsafe { method(o, 2) };
    release(o);
}

/// TEXTMETRICW (wingdi.h): what a GDI-compatible layout must agree with.
#[repr(C)]
#[derive(Clone, Copy)]
struct TextMetricW {
    height: i32,
    ascent: i32,
    descent: i32,
    internal_leading: i32,
    external_leading: i32,
    ave_char_width: i32,
    max_char_width: i32,
    weight: i32,
    overhang: i32,
    digitized_aspect_x: i32,
    digitized_aspect_y: i32,
    first_char: u16,
    last_char: u16,
    default_char: u16,
    break_char: u16,
    italic: u8,
    underlined: u8,
    struck_out: u8,
    pitch_and_family: u8,
    char_set: u8,
}

impl TextMetricW {
    const ZERO: TextMetricW = TextMetricW {
        height: 0,
        ascent: 0,
        descent: 0,
        internal_leading: 0,
        external_leading: 0,
        ave_char_width: 0,
        max_char_width: 0,
        weight: 0,
        overhang: 0,
        digitized_aspect_x: 0,
        digitized_aspect_y: 0,
        first_char: 0,
        last_char: 0,
        default_char: 0,
        break_char: 0,
        italic: 0,
        underlined: 0,
        struck_out: 0,
        pitch_and_family: 0,
        char_set: 0,
    };
}

/// DWRITE_MATRIX: a GDI-compatible layout's transform.
#[repr(C)]
#[derive(Clone, Copy)]
struct Matrix {
    m11: f32,
    m12: f32,
    m21: f32,
    m22: f32,
    dx: f32,
    dy: f32,
}

/// DWRITE_TEXT_METRICS: the text's box inside the layout's box.
#[repr(C)]
#[derive(Clone, Copy)]
struct TextMetrics {
    left: f32,
    top: f32,
    width: f32,
    width_with_trailing: f32,
    height: f32,
    layout_width: f32,
    layout_height: f32,
    max_bidi_depth: u32,
    line_count: u32,
}

impl TextMetrics {
    const ZERO: TextMetrics = TextMetrics {
        left: 0.0,
        top: 0.0,
        width: 0.0,
        width_with_trailing: 0.0,
        height: 0.0,
        layout_width: 0.0,
        layout_height: 0.0,
        max_bidi_depth: 0,
        line_count: 0,
    };
}

/// DWRITE_LINE_METRICS: one line of a laid-out text.
#[repr(C)]
#[derive(Clone, Copy)]
struct LineMetrics {
    length: u32,
    trailing_whitespace: u32,
    newline_length: u32,
    height: f32,
    baseline: f32,
    is_trimmed: i32,
}

impl LineMetrics {
    const ZERO: LineMetrics = LineMetrics {
        length: 0,
        trailing_whitespace: 0,
        newline_length: 0,
        height: 0.0,
        baseline: 0.0,
        is_trimmed: 0,
    };
}

/// DWRITE_CLUSTER_METRICS: one unbreakable piece of a laid-out text,
/// whose five flags share one `UINT16` of bit fields.
#[repr(C)]
#[derive(Clone, Copy)]
struct ClusterMetrics {
    width: f32,
    length: u16,
    flags: u16,
}

impl ClusterMetrics {
    const ZERO: ClusterMetrics = ClusterMetrics { width: 0.0, length: 0, flags: 0 };

    /// canWrapLineAfter (bit 0): a line may end here.
    fn can_wrap_after(self) -> bool {
        self.flags & 1 != 0
    }
    /// isWhitespace (bit 1).
    fn whitespace(self) -> bool {
        self.flags & 2 != 0
    }
    /// isNewline (bit 2): the cluster is a line break of its own.
    fn newline(self) -> bool {
        self.flags & 4 != 0
    }
    /// isSoftHyphen (bit 3).
    fn soft_hyphen(self) -> bool {
        self.flags & 8 != 0
    }
    /// isRightToLeft (bit 4).
    fn right_to_left(self) -> bool {
        self.flags & 16 != 0
    }
}

/// DWRITE_OVERHANG_METRICS: how far the ink reaches past each side of
/// the layout's box (positive is outside it).
#[repr(C)]
#[derive(Clone, Copy)]
struct Overhang {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl Overhang {
    const ZERO: Overhang = Overhang { left: 0.0, top: 0.0, right: 0.0, bottom: 0.0 };
}

/// DWRITE_HIT_TEST_METRICS: a box of text a hit test reports.
#[repr(C)]
#[derive(Clone, Copy)]
struct HitTestMetrics {
    text_position: u32,
    length: u32,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    bidi_level: u32,
    is_text: i32,
    is_trimmed: i32,
}

impl HitTestMetrics {
    const ZERO: HitTestMetrics = HitTestMetrics {
        text_position: 0,
        length: 0,
        left: 0.0,
        top: 0.0,
        width: 0.0,
        height: 0.0,
        bidi_level: 0,
        is_text: 0,
        is_trimmed: 0,
    };
}

/// DWRITE_TEXT_RANGE, which every ranged attribute takes by value.
#[repr(C)]
#[derive(Clone, Copy)]
struct Range {
    start: u32,
    length: u32,
}

impl Range {
    const ZERO: Range = Range { start: 0, length: 0 };
}

/// DWRITE_INLINE_OBJECT_METRICS: what an inline object (the ellipsis
/// trimming sign here) takes up in a line.
#[repr(C)]
#[derive(Clone, Copy)]
struct InlineMetrics {
    width: f32,
    height: f32,
    baseline: f32,
    supports_sideways: i32,
}

impl InlineMetrics {
    const ZERO: InlineMetrics =
        InlineMetrics { width: 0.0, height: 0.0, baseline: 0.0, supports_sideways: 0 };
}

/// Whether two measurements are the same number, allowing for the last
/// bit of a float: every comparison here is of whole pixels or of a
/// product of them, so a hundredth of a pixel is slack and not fit.
fn near(a: f32, b: f32) -> bool {
    let d = a - b;
    d < 0.01 && d > -0.01
}

/// GDI's own numbers for a family at a pixel height, through a memory
/// device context: the line's height (tmHeight), its baseline
/// (tmAscent) and the text's width (GetTextExtentPoint32W). This is what
/// a GDI-compatible layout must agree with, and the only thing it can be
/// compared against: on Windows GDI's numbers come from the face's
/// hinted metrics and are not the design values scaled (Arial at 24 px
/// reports an ascent of 21 where ascender + lineGap scale to 22.51), so
/// a formula fitted to the design values would hold nowhere.
///
/// # Safety
/// `dc` is a device context; `family` and `text` are NUL-terminated.
unsafe fn gdi_text(
    dc: *mut c_void,
    family: *const u16,
    height: i32,
    text: *const u16,
    len: i32,
    n: u32,
) -> (i32, i32, i32) {
    // SAFETY: as the caller promises; GDI's own structures.
    unsafe {
        let mut lf = LogFont::EMPTY;
        // A negative height asks for the em size in pixels, which is
        // what a format's font size is. DEFAULT_CHARSET and
        // OUT_OUTLINE_PRECIS are what the GDI interop object's own
        // LOGFONT carries (step 50).
        lf.height = height;
        lf.charset = 1;
        lf.out_precision = 8;
        let mut i = 0;
        while i < 31 && *family.add(i) != 0 {
            lf.face_name[i] = *family.add(i);
            i += 1;
        }
        let font = CreateFontIndirectW(&lf);
        check(!font.is_null(), n);
        let old = SelectObject(dc, font);
        let mut tm = TextMetricW::ZERO;
        check(GetTextMetricsW(dc, &mut tm) != 0, n);
        let mut extent = [0i32; 2];
        check(GetTextExtentPoint32W(dc, text, len, &mut extent) != 0, n);
        SelectObject(dc, old);
        DeleteObject(font);
        (tm.height, tm.ascent, extent[0])
    }
}

fn tests() {
    // SAFETY (whole function): documented COM calls through each
    // interface's vtable (slot numbers from dwrite.h), valid arguments.
    unsafe {
        // 1.
        let mut factory: *mut c_void = core::ptr::null_mut();
        check(DWriteCreateFactory(0, &IID_FACTORY, &mut factory) == 0 && !factory.is_null(), 1);

        // 2. IDWriteFactory::GetSystemFontCollection (3).
        let mut collection: *mut c_void = core::ptr::null_mut();
        let get_collection: extern "system" fn(*mut c_void, *mut *mut c_void, i32) -> HResult =
            method(factory, 3);
        check(get_collection(factory, &mut collection, 0) == 0 && !collection.is_null(), 2);
        let count: GetU32 = method(collection, 3);
        check(count(collection) >= 1, 2);
        let find: extern "system" fn(*mut c_void, *const u16, *mut u32, *mut i32) -> HResult =
            method(collection, 5);
        let (mut name, mut index, mut exists) = ([0u16; 64], 0u32, 0i32);
        check(find(collection, w("Arial", &mut name), &mut index, &mut exists) == 0, 2);
        check(exists != 0, 2);
        let mut other = 0i32;
        let mut ignored = 0u32;
        check(
            find(collection, w("No Such Family 42", &mut name), &mut ignored, &mut other) == 0,
            2,
        );
        check(other == 0, 2);

        // 3. The family (GetFontFamily, 4), its names (6) and regular font
        // (GetFirstMatchingFont, 7).
        let mut family: *mut c_void = core::ptr::null_mut();
        let get_family: GetIndexed = method(collection, 4);
        check(get_family(collection, index, &mut family) == 0 && !family.is_null(), 3);
        let mut names: *mut c_void = core::ptr::null_mut();
        let family_names: GetPtr = method(family, 6);
        check(family_names(family, &mut names) == 0, 3);
        let strings_count: GetU32 = method(names, 3);
        check(strings_count(names) >= 1, 3);
        let mut length = 0u32;
        let string_length: extern "system" fn(*mut c_void, u32, *mut u32) -> HResult =
            method(names, 7);
        check(string_length(names, 0, &mut length) == 0 && length > 0, 3);
        let first: extern "system" fn(*mut c_void, u32, u32, u32, *mut *mut c_void) -> HResult =
            method(family, 7);
        let mut font: *mut c_void = core::ptr::null_mut();
        check(first(family, 400, 5, 0, &mut font) == 0 && !font.is_null(), 3);
        let weight: GetU32 = method(font, 4);
        let stretch: GetU32 = method(font, 5);
        let style: GetU32 = method(font, 6);
        check(weight(font) == 400 && stretch(font) == 5 && style(font) == 0, 3);
        let has: extern "system" fn(*mut c_void, u32, *mut i32) -> HResult = method(font, 12);
        let mut has_a = 0;
        check(has(font, u32::from(b'A'), &mut has_a) == 0 && has_a != 0, 3);

        // 4. The face (IDWriteFont::CreateFontFace, 13).
        let mut face: *mut c_void = core::ptr::null_mut();
        let create_face: GetPtr = method(font, 13);
        check(create_face(font, &mut face) == 0 && !face.is_null(), 4);
        let face_type: GetU32 = method(face, 3);
        check(face_type(face) == 1, 4); // DWRITE_FONT_FACE_TYPE_TRUETYPE
        let mut metrics = [0u16; 10];
        let get_metrics: extern "system" fn(*mut c_void, *mut [u16; 10]) = method(face, 8);
        get_metrics(face, &mut metrics);
        check(metrics[0] >= 16 && metrics[1] > 0 && metrics[2] > 0, 4);
        let indices: extern "system" fn(*mut c_void, *const u32, u32, *mut u16) -> HResult =
            method(face, 11);
        let a = u32::from(b'A');
        let mut glyph = 0u16;
        check(indices(face, &a, 1, &mut glyph) == 0 && glyph != 0, 4);
        let glyph_metrics: extern "system" fn(
            *mut c_void,
            *const u16,
            u32,
            *mut [i32; 7],
            i32,
        ) -> HResult = method(face, 10);
        let mut gm = [0i32; 7];
        check(glyph_metrics(face, &glyph, 1, &mut gm, 0) == 0 && gm[1] > 0, 4);
        let table: extern "system" fn(
            *mut c_void,
            u32,
            *mut *const u8,
            *mut u32,
            *mut *mut c_void,
            *mut i32,
        ) -> HResult = method(face, 12);
        let cmap = u32::from_le_bytes(*b"cmap");
        let (mut data, mut size, mut context, mut found) =
            (core::ptr::null(), 0u32, core::ptr::null_mut(), 0i32);
        check(table(face, cmap, &mut data, &mut size, &mut context, &mut found) == 0, 4);
        check(found != 0 && size > 4 && !data.is_null(), 4);
        let release_table: extern "system" fn(*mut c_void, *mut c_void) = method(face, 13);
        release_table(face, context);

        // 5. The file (GetFiles, 4), its key (3) and loader (4).
        let files: extern "system" fn(*mut c_void, *mut u32, *mut *mut c_void) -> HResult =
            method(face, 4);
        let mut n = 0u32;
        check(files(face, &mut n, core::ptr::null_mut()) == 0 && n == 1, 5);
        let mut file: *mut c_void = core::ptr::null_mut();
        check(files(face, &mut n, &mut file) == 0 && !file.is_null(), 5);
        let key_of: extern "system" fn(*mut c_void, *mut *const c_void, *mut u32) -> HResult =
            method(file, 3);
        let (mut key, mut key_size) = (core::ptr::null(), 0u32);
        check(key_of(file, &mut key, &mut key_size) == 0, 5);
        let mut loader: *mut c_void = core::ptr::null_mut();
        let get_loader: GetPtr = method(file, 4);
        check(get_loader(file, &mut loader) == 0 && !loader.is_null(), 5);
        let qi: extern "system" fn(*mut c_void, *const Iid, *mut *mut c_void) -> HResult =
            method(loader, 0);
        let mut local: *mut c_void = core::ptr::null_mut();
        check(qi(loader, &IID_LOCAL_LOADER, &mut local) == 0 && !local.is_null(), 5);
        let path_of: extern "system" fn(*mut c_void, *const c_void, u32, *mut u16, u32) -> HResult =
            method(local, 5);
        let mut path = [0u16; 300];
        check(path_of(local, key, key_size, path.as_mut_ptr(), 300) == 0, 5);
        let len = path.iter().position(|&c| c == 0).unwrap_or(0);
        let ext: [u16; 4] = core::array::from_fn(|i| u16::from(b".ttf"[i]));
        let tail: [u16; 4] =
            core::array::from_fn(|i| path[len.saturating_sub(4) + i] | (0x20 * u16::from(i > 0)));
        check(len > 4 && tail == ext, 5);
        let stream_of: extern "system" fn(
            *mut c_void,
            *const c_void,
            u32,
            *mut *mut c_void,
        ) -> HResult = method(local, 3);
        let mut stream: *mut c_void = core::ptr::null_mut();
        check(stream_of(local, key, key_size, &mut stream) == 0 && !stream.is_null(), 5);
        let stream_size: extern "system" fn(*mut c_void, *mut u64) -> HResult = method(stream, 5);
        let mut file_size = 0u64;
        check(stream_size(stream, &mut file_size) == 0 && file_size > u64::from(size), 5);

        // 6. GetFontFromFontFace (6).
        let from_face: extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> HResult =
            method(collection, 6);
        let mut again: *mut c_void = core::ptr::null_mut();
        check(from_face(collection, face, &mut again) == 0 && !again.is_null(), 6);
        check(weight(again) == 400, 6);

        // 7. The file's bytes (the local stream's fragment, slot 3) back
        // the test's own stream.
        let fragment: extern "system" fn(
            *mut c_void,
            *mut *const u8,
            u64,
            u64,
            *mut *mut c_void,
        ) -> HResult = method(stream, 3);
        let (mut bytes, mut fragment_context) = (core::ptr::null(), core::ptr::null_mut());
        check(fragment(stream, &mut bytes, 0, file_size, &mut fragment_context) == 0, 7);
        custom::DATA.store(bytes.cast_mut(), core::sync::atomic::Ordering::Relaxed);
        custom::LEN.store(file_size as usize, core::sync::atomic::Ordering::Relaxed);
        custom::FACTORY.store(factory, core::sync::atomic::Ordering::Relaxed);
        // RegisterFontFileLoader (13), RegisterFontCollectionLoader (5).
        let register: GetPtr1 = method(factory, 13);
        check(register(factory, custom::file_loader()) == 0, 7);
        let register: GetPtr1 = method(factory, 5);
        check(register(factory, custom::collection_loader()) == 0, 7);
        // CreateCustomFontCollection (4).
        let create: extern "system" fn(
            *mut c_void,
            *mut c_void,
            *const c_void,
            u32,
            *mut *mut c_void,
        ) -> HResult = method(factory, 4);
        let key = 7u32;
        let mut custom_collection: *mut c_void = core::ptr::null_mut();
        let hr = create(
            factory,
            custom::collection_loader(),
            (&raw const key).cast(),
            4,
            &mut custom_collection,
        );
        check(hr == 0 && !custom_collection.is_null(), 7);
        check(count(custom_collection) == 1, 7);
        let mut custom_family: *mut c_void = core::ptr::null_mut();
        let get_family: GetIndexed = method(custom_collection, 4);
        check(get_family(custom_collection, 0, &mut custom_family) == 0, 7);
        let font_count: GetU32 = method(custom_family, 4);
        check(font_count(custom_family) >= 1, 7);
        let get_font: GetIndexed = method(custom_family, 5);
        let mut custom_font: *mut c_void = core::ptr::null_mut();
        check(get_font(custom_family, 0, &mut custom_font) == 0 && !custom_font.is_null(), 7);
        let mut custom_face: *mut c_void = core::ptr::null_mut();
        let create_face: GetPtr = method(custom_font, 13);
        check(create_face(custom_font, &mut custom_face) == 0 && !custom_face.is_null(), 7);
        let glyphs: extern "system" fn(*mut c_void) -> u16 = method(face, 9);
        let custom_glyphs: extern "system" fn(*mut c_void) -> u16 = method(custom_face, 9);
        check(glyphs(face) > 0 && custom_glyphs(custom_face) == glyphs(face), 7);
        let release_fragment: extern "system" fn(*mut c_void, *mut c_void) = method(stream, 4);
        for o in [custom_face, custom_font, custom_family, custom_collection] {
            let release: GetU32 = method(o, 2);
            release(o);
        }
        // UnregisterFontCollectionLoader (6), UnregisterFontFileLoader (14).
        let unregister: GetPtr1 = method(factory, 6);
        check(unregister(factory, custom::collection_loader()) == 0, 7);
        let unregister: GetPtr1 = method(factory, 14);
        check(unregister(factory, custom::file_loader()) == 0, 7);
        release_fragment(stream, fragment_context);

        // 8. CreateGlyphRunAnalysis (23): DWRITE_GLYPH_RUN (face, em
        // size, count, indices, advances, offsets, sideways, bidi level).
        #[repr(C)]
        struct GlyphRun {
            face: *mut c_void,
            em: f32,
            count: u32,
            indices: *const u16,
            advances: *const f32,
            offsets: *const c_void,
            sideways: i32,
            bidi: u32,
        }
        let run = GlyphRun {
            face,
            em: 32.0,
            count: 1,
            indices: &glyph,
            advances: core::ptr::null(),
            offsets: core::ptr::null(),
            sideways: 0,
            bidi: 0,
        };
        let analyze: extern "system" fn(
            *mut c_void,
            *const GlyphRun,
            f32,
            *const c_void,
            u32,
            u32,
            f32,
            f32,
            *mut *mut c_void,
        ) -> HResult = method(factory, 23);
        let mut analysis: *mut c_void = core::ptr::null_mut();
        // NATURAL (4), measuring NATURAL (0), baseline at (10, 40).
        let hr = analyze(factory, &run, 1.0, core::ptr::null(), 4, 0, 10.0, 40.0, &mut analysis);
        check(hr == 0 && !analysis.is_null(), 8);
        let bounds: extern "system" fn(*mut c_void, u32, *mut [i32; 4]) -> HResult =
            method(analysis, 3);
        let mut aliased = [1i32; 4];
        check(bounds(analysis, 0, &mut aliased) == 0, 8);
        check(aliased[0] >= aliased[2] || aliased[1] >= aliased[3], 8);
        let mut rect = [0i32; 4];
        check(bounds(analysis, 1, &mut rect) == 0, 8);
        let (width, height) = (rect[2] - rect[0], rect[3] - rect[1]);
        check(width > 0 && height > 0 && height <= 33 && rect[3] <= 41 && rect[0] >= 9, 8);
        let texture: extern "system" fn(
            *mut c_void,
            u32,
            *const [i32; 4],
            *mut u8,
            u32,
        ) -> HResult = method(analysis, 4);
        // One thread; the only reference to ALPHA (inside the function's
        // unsafe block).
        let alpha = core::slice::from_raw_parts_mut((&raw mut ALPHA).cast::<u8>(), 3 * 48 * 48);
        let size = (3 * width * height) as u32;
        check(size as usize <= alpha.len(), 8);
        check(texture(analysis, 1, &rect, alpha.as_mut_ptr(), size) == 0, 8);
        check(alpha[..size as usize].iter().any(|&a| a > 0), 8);
        // A buffer too small is E_NOT_SUFFICIENT_BUFFER.
        check(texture(analysis, 1, &rect, alpha.as_mut_ptr(), size - 1) != 0, 8);
        let release: GetU32 = method(analysis, 2);
        release(analysis);
        // ALIASED (1): an aliased texture of 0 and 255.
        let mut analysis: *mut c_void = core::ptr::null_mut();
        let hr = analyze(factory, &run, 1.0, core::ptr::null(), 1, 0, 10.0, 40.0, &mut analysis);
        check(hr == 0 && !analysis.is_null(), 8);
        let bounds: extern "system" fn(*mut c_void, u32, *mut [i32; 4]) -> HResult =
            method(analysis, 3);
        let mut clear_type = [1i32; 4];
        check(bounds(analysis, 1, &mut clear_type) == 0, 8);
        check(clear_type[0] >= clear_type[2] || clear_type[1] >= clear_type[3], 8);
        check(bounds(analysis, 0, &mut rect) == 0, 8);
        let (width, height) = (rect[2] - rect[0], rect[3] - rect[1]);
        check(width > 0 && height > 0, 8);
        let texture: extern "system" fn(
            *mut c_void,
            u32,
            *const [i32; 4],
            *mut u8,
            u32,
        ) -> HResult = method(analysis, 4);
        let size = (width * height) as u32;
        check(texture(analysis, 0, &rect, alpha.as_mut_ptr(), size) == 0, 8);
        let used = &alpha[..size as usize];
        check(used.contains(&255) && used.iter().all(|&a| a == 0 || a == 255), 8);
        let release: GetU32 = method(analysis, 2);
        release(analysis);
        // OUTLINE (6) cannot be analyzed.
        let mut none: *mut c_void = core::ptr::null_mut();
        let hr = analyze(factory, &run, 1.0, core::ptr::null(), 6, 0, 0.0, 0.0, &mut none);
        check(hr == 0x8007_0057_u32 as i32 && none.is_null(), 8);

        // 9. GetGlyphRunOutline (14) into the test's sink.
        let outline: extern "system" fn(
            *mut c_void,
            f32,
            *const u16,
            *const f32,
            *const c_void,
            u32,
            i32,
            i32,
            *mut c_void,
        ) -> HResult = method(face, 14);
        let hr = outline(
            face,
            32.0,
            &glyph,
            core::ptr::null(),
            core::ptr::null(),
            1,
            0,
            0,
            sink::sink(),
        );
        check(hr == 0, 9);
        let figures = sink::FIGURES.load(core::sync::atomic::Ordering::Relaxed);
        let ended = sink::ENDED.load(core::sync::atomic::Ordering::Relaxed);
        let segments = sink::SEGMENTS.load(core::sync::atomic::Ordering::Relaxed);
        check(figures >= 1 && ended == figures && segments >= 3, 9);
        check(sink::ABOVE.load(core::sync::atomic::Ordering::Relaxed), 9);
        check(sink::CLOSED.load(core::sync::atomic::Ordering::Relaxed) == 0, 9);

        // 10. Text-rendering parameters: the factory's own set
        // (CreateRenderingParams, 10).
        let make_params: GetPtr = method(factory, 10);
        let mut params: *mut c_void = core::ptr::null_mut();
        check(make_params(factory, &mut params) == 0 && !params.is_null(), 10);
        let (gamma, contrast, cleartype, geometry, mode, grayscale, grid_fit, mode1) =
            values_of(params);
        // The gamma and the contrasts are the display's, so only their
        // ranges are Windows' everywhere; the modes are not a display's.
        check(gamma > 0.0 && gamma <= 256.0, 10);
        check(contrast >= 0.0 && grayscale >= 0.0, 10);
        check(cleartype == 1.0, 10);
        check(geometry <= 2, 10);
        check(mode == 0 && mode1 == 0 && grid_fit == 0, 10);
        // One object, four interfaces.
        let params_qi: extern "system" fn(*mut c_void, *const Iid, *mut *mut c_void) -> HResult =
            method(params, 0);
        for iid in [IID_PARAMS1, IID_PARAMS2, IID_PARAMS3] {
            let mut same: *mut c_void = core::ptr::null_mut();
            check(params_qi(params, &iid, &mut same) == 0 && same == params, 10);
            let release: GetU32 = method(same, 2);
            release(same);
        }
        // A monitor's set (CreateMonitorRenderingParams, 11): the same
        // values, a new object. Windows takes a NULL monitor.
        let monitor_params: GetIndexed = method(factory, 11);
        let mut other: *mut c_void = core::ptr::null_mut();
        check(monitor_params(factory, 0, &mut other) == 0 && !other.is_null(), 10);
        check(other != params, 10);
        check(
            values_of(other)
                == (gamma, contrast, cleartype, geometry, mode, grayscale, grid_fit, mode1),
            10,
        );
        let release: GetU32 = method(other, 2);
        release(other);
        // A new object per call.
        let mut again_params: *mut c_void = core::ptr::null_mut();
        check(make_params(factory, &mut again_params) == 0 && again_params != params, 10);
        let release: GetU32 = method(again_params, 2);
        release(again_params);
        // A custom set (CreateCustomRenderingParams, 12) holds exactly
        // what it was made with; the grayscale contrast and the grid fit
        // mode stay the system's.
        let custom_params: MakeCustom = method(factory, 12);
        let mut mine: *mut c_void = core::ptr::null_mut();
        check(custom_params(factory, 2.2, 0.75, 0.25, 2, 5, &mut mine) == 0, 10);
        check(values_of(mine) == (2.2, 0.75, 0.25, 2, 5, grayscale, grid_fit, 5), 10);
        let release: GetU32 = method(mine, 2);
        release(mine);
        // What it refuses, and the edges it does not: a contrast or a
        // ClearType level above 1 is taken, and so is the largest gamma.
        const E_INVALIDARG: HResult = 0x8007_0057_u32 as i32;
        for (gamma, contrast, cleartype, geometry, mode) in [
            (0.0, 1.0, 1.0, 1, 0),
            (-1.0, 1.0, 1.0, 1, 0),
            (256.1, 1.0, 1.0, 1, 0),
            (1.8, -1.0, 1.0, 1, 0),
            (1.8, 1.0, -1.0, 1, 0),
            (1.8, 1.0, 1.0, 3, 0),
            (1.8, 1.0, 1.0, 1, 7),
        ] {
            let mut none: *mut c_void = core::ptr::null_mut();
            let hr = custom_params(factory, gamma, contrast, cleartype, geometry, mode, &mut none);
            check(hr == E_INVALIDARG && none.is_null(), 10);
        }
        for (gamma, contrast, cleartype) in [(256.0, 1.0, 1.0), (1.8, 2.0, 2.0)] {
            let mut taken: *mut c_void = core::ptr::null_mut();
            check(custom_params(factory, gamma, contrast, cleartype, 1, 0, &mut taken) == 0, 10);
            check(!taken.is_null(), 10);
            let release: GetU32 = method(taken, 2);
            release(taken);
        }
        let release: GetU32 = method(params, 2);
        release(params);

        // 11. The later factory revisions: Windows answers every one of
        // them with this same factory object, by either route; gala
        // refuses each until that revision's methods exist, and says so
        // with the whole IID (the DLL's FACTORY_IIDS explains why). This
        // program is the parity test and runs on Windows as well
        // (user/windows/ci-passing.txt), so it takes either answer and
        // insists only that each route wrote the slot; which one gala
        // gives is pinned by the dwrite_test scenario instead, on the
        // refusal's own ledger row.
        const E_NOINTERFACE: HResult = 0x8000_4002_u32 as i32;
        let factory_qi: extern "system" fn(*mut c_void, *const Iid, *mut *mut c_void) -> HResult =
            method(factory, 0);
        for iid in IID_FACTORY_REVISIONS {
            for route in 0..2 {
                // A pointer that is neither answer, so either branch
                // proves the call wrote it.
                let mut got: *mut c_void = collection;
                let hr = if route == 0 {
                    factory_qi(factory, &iid, &mut got)
                } else {
                    DWriteCreateFactory(0, &iid, &mut got)
                };
                if hr == 0 {
                    // Windows: this same factory, one reference more.
                    check(got == factory, 11);
                    let release: GetU32 = method(got, 2);
                    release(got);
                } else {
                    check(hr == E_NOINTERFACE && got.is_null(), 11);
                }
            }
        }
        // IDWriteFactory::GetGdiInterop (17): the factory's own object,
        // the same one on a second call, which answers both GDI interop
        // interfaces as itself.
        let gdi_interop: GetPtr = method(factory, 17);
        let mut interop: *mut c_void = core::ptr::null_mut();
        check(gdi_interop(factory, &mut interop) == 0 && !interop.is_null(), 11);
        check(interop != factory, 11);
        let mut interop_again: *mut c_void = core::ptr::null_mut();
        check(gdi_interop(factory, &mut interop_again) == 0 && interop_again == interop, 11);
        let release: GetU32 = method(interop_again, 2);
        release(interop_again);
        let interop_qi: extern "system" fn(*mut c_void, *const Iid, *mut *mut c_void) -> HResult =
            method(interop, 0);
        for iid in [IID_GDI_INTEROP, IID_GDI_INTEROP1, IID_UNKNOWN] {
            let mut same: *mut c_void = core::ptr::null_mut();
            check(interop_qi(interop, &iid, &mut same) == 0 && same == interop, 11);
            let release: GetU32 = method(same, 2);
            release(same);
        }
        let release: GetU32 = method(interop, 2);
        release(interop);

        // 12. Text analysis. Word's startup stops here without it: the
        // DLL's ledger shows CreateTextAnalyzer refused 13 ms before the
        // error box, so this step pins one analyzer's whole job on
        // "Hello world 123" -- the object and its three revisions, what the
        // four Analyze* methods report through a sink, how little of the
        // source they read, and the glyphs and advances the two shaping
        // calls give. Windows makes a fresh analyzer per call and answers
        // IUnknown with a third pointer; gala has one static analyzer and
        // answers IUnknown with itself, so what is insisted on here is only
        // that both wrote a pointer that is not the factory.
        let mut text_buf = [0u16; 64];
        let text = w("Hello world 123", &mut text_buf);
        let mut locale_buf = [0u16; 64];
        let locale = w("en-us", &mut locale_buf);
        const TEXT_LEN: u32 = 15;

        let make_analyzer: GetPtr = method(factory, 21);
        let mut analyzer: *mut c_void = core::ptr::null_mut();
        check(
            make_analyzer(factory, &mut analyzer) == 0
                && !analyzer.is_null()
                && analyzer != factory,
            24,
        );

        let add_ref: GetU32 = method(analyzer, 1);
        let release: GetU32 = method(analyzer, 2);
        check(add_ref(analyzer) == 2 && release(analyzer) == 1, 25);

        // One pointer, three revisions: there is no IDWriteTextAnalyzer3.
        let analyzer_qi: extern "system" fn(*mut c_void, *const Iid, *mut *mut c_void) -> HResult =
            method(analyzer, 0);
        for iid in [IID_ANALYZER, IID_ANALYZER1, IID_ANALYZER2] {
            let mut same: *mut c_void = core::ptr::null_mut();
            check(analyzer_qi(analyzer, &iid, &mut same) == 0 && same == analyzer, 26);
            let release: GetU32 = method(same, 2);
            release(same);
        }
        let mut unknown: *mut c_void = core::ptr::null_mut();
        check(analyzer_qi(analyzer, &IID_UNKNOWN, &mut unknown) == 0 && !unknown.is_null(), 27);
        let release_unknown: GetU32 = method(unknown, 2);
        release_unknown(unknown);
        // Anything else is refused with the slot nulled, not left alone.
        let mut none: *mut c_void = analyzer;
        check(
            analyzer_qi(analyzer, &IID_FACTORY, &mut none) == E_NOINTERFACE && none.is_null(),
            28,
        );

        // One Latin script run over the whole string: script 49, shapes
        // DEFAULT (0).
        let analyze_script: Analyze = method(analyzer, 3);
        analysis::arm(text, TEXT_LEN);
        check(analyze_script(analyzer, analysis::source(), 0, TEXT_LEN, analysis::sink()) == 0, 29);
        check(analysis::script() == [1, 0, TEXT_LEN, 49, 0], 29);
        // The source is barely consulted: GetTextAtPosition once, at
        // position 0, and the other four methods not at all.
        check(analysis::source_calls() == [1, 0, 0, 0, 0, 0], 30);

        // One bidi run, explicit and resolved level 0.
        let analyze_bidi: Analyze = method(analyzer, 4);
        analysis::arm(text, TEXT_LEN);
        check(analyze_bidi(analyzer, analysis::source(), 0, TEXT_LEN, analysis::sink()) == 0, 31);
        check(analysis::bidi() == [1, 0, TEXT_LEN, 0, 0], 31);

        // Number substitution over a string with digits in it: S_OK and the
        // sink never called at all. A silent success is the contract.
        let analyze_subst: Analyze = method(analyzer, 5);
        analysis::arm(text, TEXT_LEN);
        check(analyze_subst(analyzer, analysis::source(), 0, TEXT_LEN, analysis::sink()) == 0, 32);
        check(analysis::subst_runs() == 0, 32);

        // One breakpoint call covering every character.
        let analyze_breaks: Analyze = method(analyzer, 6);
        analysis::arm(text, TEXT_LEN);
        check(analyze_breaks(analyzer, analysis::source(), 0, TEXT_LEN, analysis::sink()) == 0, 33);
        check(analysis::breaks() == [1, 0, TEXT_LEN], 33);
        // Bits 0-1 are the condition before, 2-3 the condition after, 4 is
        // isWhitespace; 2 may not break, 1 may. So a letter is 2/2, each
        // space 2/1 with isWhitespace on the space itself, the character
        // after a space 1/2, and the last character 2/1. Bits 5-7
        // (isSoftHyphen and padding) were not measured, so they are masked
        // off rather than guessed at.
        let want = [10u8, 10, 10, 10, 10, 22, 9, 10, 10, 10, 10, 22, 9, 10, 6];
        for (i, b) in want.iter().enumerate() {
            check(analysis::break_at(i) & 0b1_1111 == *b, 34);
        }

        // GetGlyphs: 15 characters to 15 glyphs, the cluster map the
        // identity.
        let get_glyphs: GetGlyphs = method(analyzer, 7);
        let sa = ScriptAnalysis { script: 49, shapes: 0 };
        let mut clustermap = [0u16; 32];
        let mut text_props = [0u16; 32];
        let mut glyph_props = [0u16; 32];
        let mut glyphs = [0u16; 32];
        let mut count = 0u32;
        check(
            get_glyphs(
                analyzer,
                text,
                TEXT_LEN,
                face,
                0,
                0,
                &sa,
                locale,
                core::ptr::null_mut(),
                core::ptr::null(),
                core::ptr::null(),
                0,
                32,
                clustermap.as_mut_ptr(),
                text_props.as_mut_ptr(),
                glyphs.as_mut_ptr(),
                glyph_props.as_mut_ptr(),
                &mut count,
            ) == 0
                && count == TEXT_LEN,
            35,
        );
        for i in 0..TEXT_LEN as usize {
            check(clustermap[i] == i as u16, 35);
            // Bit 4 isClusterStart, 5 isDiacritic, 6 isZeroWidthSpace.
            check(glyph_props[i] & 0b111_0000 == 0b001_0000, 36);
        }
        // Both spaces are the one glyph the face itself gives for a space,
        // so the mapping is the font's and not a table of our own.
        let space = u32::from(b' ');
        let mut space_glyph = 0u16;
        check(
            indices(face, &space, 1, &mut space_glyph) == 0
                && glyphs[5] == space_glyph
                && glyphs[11] == space_glyph,
            36,
        );
        // Justification is bits 0-3: 2 for a letter, 4 for the space.
        check(glyph_props[0] & 0b1111 == 2 && glyph_props[5] & 0b1111 == 4, 37);

        // GetGlyphPlacements at em size 16: each advance is the glyph's own
        // design advance scaled by the em size, which the face can be asked
        // for, so the numbers hold whatever font is installed. Every offset
        // is zero.
        let get_placements: GetPlacements = method(analyzer, 8);
        let mut advances = [0f32; 32];
        let mut offsets = [[0f32; 2]; 32];
        check(
            get_placements(
                analyzer,
                text,
                clustermap.as_ptr(),
                text_props.as_mut_ptr(),
                TEXT_LEN,
                glyphs.as_ptr(),
                glyph_props.as_ptr(),
                count,
                face,
                16.0,
                0,
                0,
                &sa,
                locale,
                core::ptr::null(),
                core::ptr::null(),
                0,
                advances.as_mut_ptr(),
                offsets.as_mut_ptr(),
            ) == 0,
            38,
        );
        // A pixel's thousandth: the two routes to the quotient need not
        // agree in the last bits for every design unit count.
        const EPS: f32 = 1.0 / 1024.0;
        for i in 0..TEXT_LEN as usize {
            let mut gm = [0i32; 7];
            check(glyph_metrics(face, &glyphs[i], 1, &mut gm, 0) == 0, 38);
            let d = advances[i] - gm[1] as f32 * 16.0 / f32::from(metrics[0]);
            check(d > -EPS && d < EPS, 38);
            check(offsets[i][0] == 0.0 && offsets[i][1] == 0.0, 38);
        }

        // A zero-length range is S_OK with no runs at all.
        analysis::arm(text, TEXT_LEN);
        check(analyze_script(analyzer, analysis::source(), 0, 0, analysis::sink()) == 0, 39);
        check(analysis::script()[0] == 0, 39);
        // A NULL sink or source is E_INVALIDARG and not a fault. (Two edges
        // are measured and not asserted, because a test cannot assert a
        // crash: real DirectWrite faults on CreateTextAnalyzer(NULL) and on
        // a position past the end of the source.)
        check(
            analyze_script(analyzer, analysis::source(), 0, TEXT_LEN, core::ptr::null_mut())
                == E_INVALIDARG,
            40,
        );
        check(
            analyze_script(analyzer, core::ptr::null_mut(), 0, TEXT_LEN, analysis::sink())
                == E_INVALIDARG,
            40,
        );

        let release: GetU32 = method(analyzer, 2);
        release(analyzer);

        // 13. Text formats (IDWriteFactory::CreateTextFormat, 15),
        // typography (16), number substitution (22) and the GDI interop
        // object's LOGFONT conversions (4, 5): what Word asks for as it
        // lays its window's text out.
        const E_NOT_SUFFICIENT_BUFFER: HResult = 0x8007_007a_u32 as i32;
        /// DWRITE_FONT_FEATURE_TAG_KERNING, which is the bytes "kern".
        const KERNING: u32 = u32::from_le_bytes(*b"kern");
        type MakeFormat = extern "system" fn(
            *mut c_void,
            *const u16,
            *mut c_void,
            u32,
            u32,
            u32,
            f32,
            *const u16,
            *mut *mut c_void,
        ) -> HResult;
        type SetU32 = extern "system" fn(*mut c_void, u32) -> HResult;
        type SetF32 = extern "system" fn(*mut c_void, f32) -> HResult;
        type ReadString = extern "system" fn(*mut c_void, *mut u16, u32) -> HResult;
        let make_format: MakeFormat = method(factory, 15);
        let (mut fb, mut lb, mut nb) = ([0u16; 64], [0u16; 64], [0u16; 64]);
        // The family step 2 found, and the locale every format here is
        // made with. The name is one any Windows install has, and gala's
        // collection answers it with its own family (FindFamilyName).
        let arial = w("Arial", &mut fb);
        let enus = w("en-us", &mut lb);
        let make = |family: *const u16,
                    weight: u32,
                    style: u32,
                    stretch: u32,
                    size: f32,
                    locale: *const u16|
         -> (HResult, *mut c_void) {
            let mut made: *mut c_void = core::ptr::null_mut();
            let r = make_format(
                factory,
                family,
                core::ptr::null_mut(),
                weight,
                style,
                stretch,
                size,
                locale,
                &mut made,
            );
            (r, made)
        };

        // A format keeps every value it was made with, and both strings
        // verbatim -- a format is not a lookup, and a family no
        // collection has is taken as readily (check 45). Each string
        // wants a buffer for its terminator as well: its length exactly
        // is E_NOT_SUFFICIENT_BUFFER.
        let (r, fmt) = make(arial, 400, 0, 5, 16.0, enus);
        check(r == 0 && !fmt.is_null(), 41);
        let family_name_length: GetU32 = method(fmt, 20);
        let family_name: ReadString = method(fmt, 21);
        let locale_name_length: GetU32 = method(fmt, 26);
        let locale_name: ReadString = method(fmt, 27);
        let font_weight: GetU32 = method(fmt, 22);
        let font_style: GetU32 = method(fmt, 23);
        let font_stretch: GetU32 = method(fmt, 24);
        let font_size: GetF32 = method(fmt, 25);
        check(family_name_length(fmt) == 5 && locale_name_length(fmt) == 5, 41);
        let mut read = [0u16; 64];
        check(family_name(fmt, read.as_mut_ptr(), 5) == E_NOT_SUFFICIENT_BUFFER, 41);
        check(family_name(fmt, read.as_mut_ptr(), 0) == E_NOT_SUFFICIENT_BUFFER, 41);
        check(family_name(fmt, read.as_mut_ptr(), 6) == 0 && is_text(&read, "Arial"), 41);
        check(locale_name(fmt, read.as_mut_ptr(), 5) == E_NOT_SUFFICIENT_BUFFER, 41);
        check(locale_name(fmt, read.as_mut_ptr(), 6) == 0 && is_text(&read, "en-us"), 41);
        check(font_weight(fmt) == 400 && font_style(fmt) == 0 && font_stretch(fmt) == 5, 41);
        check(font_size(fmt) == 16.0, 41);

        // A NULL collection means the system's, and the format answers
        // that same object (measured: the pointer GetSystemFontCollection
        // gives).
        let format_collection: GetPtr = method(fmt, 19);
        let mut held: *mut c_void = core::ptr::null_mut();
        check(format_collection(fmt, &mut held) == 0 && held == collection, 42);
        drop_com(held);

        // What a format starts with: leading alignment both ways, no
        // wrapping option, left-to-right and top-to-bottom, the default
        // line spacing, no trimming -- and an incremental tab stop of
        // four times the em size (measured at 16, 10, 7.5 and 0.5).
        let text_alignment: GetU32 = method(fmt, 11);
        let paragraph_alignment: GetU32 = method(fmt, 12);
        let word_wrapping: GetU32 = method(fmt, 13);
        let reading_direction: GetU32 = method(fmt, 14);
        let flow_direction: GetU32 = method(fmt, 15);
        let tab_stop: GetF32 = method(fmt, 16);
        let get_trimming: extern "system" fn(
            *mut c_void,
            *mut [u32; 3],
            *mut *mut c_void,
        ) -> HResult = method(fmt, 17);
        let get_line_spacing: extern "system" fn(
            *mut c_void,
            *mut u32,
            *mut f32,
            *mut f32,
        ) -> HResult = method(fmt, 18);
        check(text_alignment(fmt) == 0 && paragraph_alignment(fmt) == 0, 43);
        check(word_wrapping(fmt) == 0 && reading_direction(fmt) == 0, 43);
        check(flow_direction(fmt) == 0 && tab_stop(fmt) == 64.0, 43);
        let (mut spacing_method, mut spacing, mut baseline) = (9u32, 9f32, 9f32);
        check(get_line_spacing(fmt, &mut spacing_method, &mut spacing, &mut baseline) == 0, 43);
        check(spacing_method == 0 && spacing == 0.0 && baseline == 0.0, 43);
        let mut trimming = [9u32; 3];
        // A pointer that is neither answer, so the call is seen to write it.
        let mut sign: *mut c_void = fmt;
        check(get_trimming(fmt, &mut trimming, &mut sign) == 0, 43);
        check(trimming == [0, 0, 0] && sign.is_null(), 43);
        let (r, ten) = make(arial, 400, 0, 5, 10.0, enus);
        check(r == 0, 43);
        let ten_tab: GetF32 = method(ten, 16);
        check(ten_tab(ten) == 40.0, 43);
        drop_com(ten);

        // A format is a counted object of its own: every call makes
        // another, AddRef and Release count it, and it answers its own
        // interface with itself. The revisions (IDWriteTextFormat1, 2 and
        // 3) are the divergence step 11 describes -- Windows answers them
        // with this same object, gala refuses each until the methods it
        // adds exist -- so this step takes either answer and insists only
        // that the slot was written. IUnknown is not asked about at all:
        // Windows answers it with a different pointer again.
        let (r, another) = make(arial, 400, 0, 5, 16.0, enus);
        check(r == 0 && another != fmt, 44);
        drop_com(another);
        let format_qi: extern "system" fn(*mut c_void, *const Iid, *mut *mut c_void) -> HResult =
            method(fmt, 0);
        let add_ref: GetU32 = method(fmt, 1);
        let release: GetU32 = method(fmt, 2);
        let mut got: *mut c_void = core::ptr::null_mut();
        check(format_qi(fmt, &IID_TEXT_FORMAT, &mut got) == 0 && got == fmt, 44);
        drop_com(got);
        got = fmt;
        check(
            format_qi(fmt, &IID_NOT_AN_INTERFACE, &mut got) == E_NOINTERFACE && got.is_null(),
            44,
        );
        for revision in IID_TEXT_FORMAT_REVISIONS {
            got = fmt;
            let hr = format_qi(fmt, &revision, &mut got);
            if hr == 0 {
                check(!got.is_null(), 44);
                drop_com(got);
            } else {
                check(hr == E_NOINTERFACE && got.is_null(), 44);
            }
        }
        check(add_ref(fmt) == 2 && release(fmt) == 1, 44);

        // What CreateTextFormat refuses (measured). A family name is only
        // a string: empty, unknown, and a thousand characters long are
        // all taken, and NULL is not. The weight runs 1..=999, and a
        // value between the enumeration's is kept as it is; the style
        // runs 0..=2 and the stretch 1..=9; the size must be a positive
        // number.
        check(make(core::ptr::null(), 400, 0, 5, 16.0, enus).0 == E_INVALIDARG, 45);
        let mut eb = [0u16; 64];
        let empty = w("", &mut eb);
        for family in [empty, w("No Such Family 42", &mut nb)] {
            let (r, made) = make(family, 400, 0, 5, 16.0, enus);
            check(r == 0, 45);
            drop_com(made);
        }
        // One thread; the only reference to LONG_NAME.
        let long = core::slice::from_raw_parts_mut((&raw mut LONG_NAME).cast::<u16>(), 1024);
        for c in long.iter_mut().take(1000) {
            *c = u16::from(b'a');
        }
        let (r, made) = make(long.as_ptr(), 400, 0, 5, 16.0, enus);
        check(r == 0, 45);
        let long_length: GetU32 = method(made, 20);
        check(long_length(made) == 1000, 45);
        drop_com(made);
        for size in [0.0, -1.0, f32::NAN] {
            check(make(arial, 400, 0, 5, size, enus).0 == E_INVALIDARG, 45);
        }
        for size in [0.0001, 1e30] {
            let (r, made) = make(arial, 400, 0, 5, size, enus);
            check(r == 0, 45);
            drop_com(made);
        }
        for (wt, st, sr) in [(0, 0, 5), (1000, 0, 5), (400, 3, 5), (400, 0, 0), (400, 0, 10)] {
            check(make(arial, wt, st, sr, 16.0, enus).0 == E_INVALIDARG, 45);
        }
        for (wt, st, sr) in [(1, 0, 5), (999, 0, 5), (400, 2, 5), (400, 0, 1), (400, 0, 9)] {
            let (r, made) = make(arial, wt, st, sr, 16.0, enus);
            check(r == 0, 45);
            drop_com(made);
        }
        let (r, odd) = make(arial, 450, 0, 5, 16.0, enus);
        check(r == 0, 45);
        let odd_weight: GetU32 = method(odd, 22);
        check(odd_weight(odd) == 450, 45);
        drop_com(odd);
        check(make(arial, 400, 0, 5, 16.0, core::ptr::null()).0 == E_INVALIDARG, 45);
        let (r, made) = make(arial, 400, 0, 5, 16.0, empty);
        check(r == 0, 45);
        drop_com(made);

        // What a locale name may look like, which is measured and is not
        // a length: hyphen-separated runs of one to eight characters,
        // BCP-47's shape. Two runs of eight are taken where one run of
        // nine is not, and a name may not begin or end with a hyphen or
        // hold two running. What is inside a run is not looked at.
        for locale in ["a", "aaaaaaaa", "aaaaaaaa-aaaaaaaa", "en_us", "zz-not-a-locale"] {
            let (r, made) = make(arial, 400, 0, 5, 16.0, w(locale, &mut nb));
            check(r == 0, 46);
            drop_com(made);
        }
        for locale in ["aaaaaaaaa", "aaaaaaaaaaaa", "en-", "-en", "en--us"] {
            check(make(arial, 400, 0, 5, 16.0, w(locale, &mut nb)).0 == E_INVALIDARG, 46);
        }

        // The settable half: each setter takes the enumeration's values
        // and refuses the first past it, a refused call leaves the old
        // value, and every getter reads back what was set. The tab stop
        // must be positive; a uniform line spacing takes a spacing of
        // zero or more with any baseline at all; trimming takes a
        // granularity, a delimiter and a count, and no options at all is
        // E_INVALIDARG.
        let set_text_alignment: SetU32 = method(fmt, 3);
        let set_paragraph_alignment: SetU32 = method(fmt, 4);
        let set_word_wrapping: SetU32 = method(fmt, 5);
        let set_reading_direction: SetU32 = method(fmt, 6);
        let set_flow_direction: SetU32 = method(fmt, 7);
        let set_tab_stop: SetF32 = method(fmt, 8);
        let set_trimming: extern "system" fn(*mut c_void, *const [u32; 3], *mut c_void) -> HResult =
            method(fmt, 9);
        let set_line_spacing: extern "system" fn(*mut c_void, u32, f32, f32) -> HResult =
            method(fmt, 10);
        check(set_text_alignment(fmt, 2) == 0 && text_alignment(fmt) == 2, 47);
        check(set_text_alignment(fmt, 3) == 0 && text_alignment(fmt) == 3, 47);
        check(set_text_alignment(fmt, 4) == E_INVALIDARG && text_alignment(fmt) == 3, 47);
        check(set_paragraph_alignment(fmt, 2) == 0 && paragraph_alignment(fmt) == 2, 47);
        check(set_paragraph_alignment(fmt, 3) == E_INVALIDARG && paragraph_alignment(fmt) == 2, 47);
        check(set_word_wrapping(fmt, 4) == 0 && word_wrapping(fmt) == 4, 47);
        check(set_word_wrapping(fmt, 5) == E_INVALIDARG && word_wrapping(fmt) == 4, 47);
        check(set_reading_direction(fmt, 3) == 0 && reading_direction(fmt) == 3, 47);
        check(set_reading_direction(fmt, 4) == E_INVALIDARG && reading_direction(fmt) == 3, 47);
        check(set_flow_direction(fmt, 3) == 0 && flow_direction(fmt) == 3, 47);
        check(set_flow_direction(fmt, 4) == E_INVALIDARG && flow_direction(fmt) == 3, 47);
        check(set_tab_stop(fmt, 40.0) == 0 && tab_stop(fmt) == 40.0, 47);
        check(set_tab_stop(fmt, 0.0) == E_INVALIDARG && tab_stop(fmt) == 40.0, 47);
        check(set_tab_stop(fmt, -1.0) == E_INVALIDARG && tab_stop(fmt) == 40.0, 47);
        check(set_line_spacing(fmt, 1, 20.0, 16.0) == 0, 47);
        check(get_line_spacing(fmt, &mut spacing_method, &mut spacing, &mut baseline) == 0, 47);
        check(spacing_method == 1 && spacing == 20.0 && baseline == 16.0, 47);
        check(set_line_spacing(fmt, 1, -1.0, 0.0) == E_INVALIDARG, 47);
        check(set_line_spacing(fmt, 3, 20.0, 16.0) == E_INVALIDARG, 47);
        check(set_line_spacing(fmt, 7, 20.0, 16.0) == E_INVALIDARG, 47);
        check(get_line_spacing(fmt, &mut spacing_method, &mut spacing, &mut baseline) == 0, 47);
        check(spacing_method == 1 && spacing == 20.0 && baseline == 16.0, 47);
        check(set_line_spacing(fmt, 2, 20.0, 16.0) == 0, 47);
        check(set_line_spacing(fmt, 1, 0.0, 0.0) == 0, 47);
        check(set_line_spacing(fmt, 1, 20.0, -1.0) == 0, 47);
        check(set_trimming(fmt, &[1, 0, 0], core::ptr::null_mut()) == 0, 47);
        check(get_trimming(fmt, &mut trimming, &mut sign) == 0, 47);
        check(trimming == [1, 0, 0] && sign.is_null(), 47);
        check(set_trimming(fmt, core::ptr::null(), core::ptr::null_mut()) == E_INVALIDARG, 47);
        check(set_trimming(fmt, &[3, 0, 0], core::ptr::null_mut()) == E_INVALIDARG, 47);
        check(set_trimming(fmt, &[2, 0, 0], core::ptr::null_mut()) == 0, 47);
        check(set_trimming(fmt, &[1, u32::from(b','), 7], core::ptr::null_mut()) == 0, 47);
        check(get_trimming(fmt, &mut trimming, &mut sign) == 0, 47);
        check(trimming == [1, u32::from(b','), 7] && sign.is_null(), 47);
        // The read-only half is untouched by all of that.
        check(family_name_length(fmt) == 5 && font_weight(fmt) == 400, 47);
        check(font_size(fmt) == 16.0 && font_stretch(fmt) == 5, 47);
        drop_com(fmt);

        // Typography (CreateTypography, 16): an empty set of font
        // features that takes them one at a time, by tag and parameter,
        // and reads them back in the order they went in. A tag is four
        // bytes and is not looked at; an index past the end is
        // E_INVALIDARG.
        let create_typography: GetPtr = method(factory, 16);
        let mut typography: *mut c_void = core::ptr::null_mut();
        check(create_typography(factory, &mut typography) == 0 && !typography.is_null(), 48);
        let feature_count: GetU32 = method(typography, 4);
        let add_feature: extern "system" fn(*mut c_void, [u32; 2]) -> HResult =
            method(typography, 3);
        let get_feature: extern "system" fn(*mut c_void, u32, *mut [u32; 2]) -> HResult =
            method(typography, 5);
        check(feature_count(typography) == 0, 48);
        check(add_feature(typography, [KERNING, 1]) == 0 && feature_count(typography) == 1, 48);
        let mut feature = [9u32; 2];
        check(get_feature(typography, 0, &mut feature) == 0 && feature == [KERNING, 1], 48);
        check(get_feature(typography, 1, &mut feature) == E_INVALIDARG, 48);
        check(add_feature(typography, [0, 0]) == 0 && feature_count(typography) == 2, 48);
        check(get_feature(typography, 1, &mut feature) == 0 && feature == [0, 0], 48);
        check(get_feature(typography, 0, &mut feature) == 0 && feature == [KERNING, 1], 48);
        check(get_feature(typography, 2, &mut feature) == E_INVALIDARG, 48);
        drop_com(typography);

        // Number substitution (CreateNumberSubstitution, 22): a method,
        // a locale and whether the user's own override is ignored. The
        // method runs 0..=4 (FROM_CULTURE to TRADITIONAL) and the locale
        // is not looked at -- but only NONE (2) may be given no locale at
        // all, measured, the others having nothing to substitute from.
        let create_substitution: extern "system" fn(
            *mut c_void,
            u32,
            *const u16,
            i32,
            *mut *mut c_void,
        ) -> HResult = method(factory, 22);
        let mut substitution: *mut c_void = core::ptr::null_mut();
        for (wanted, locale, ignore) in [
            (2u32, enus, 0i32),
            (3, w("ar-eg", &mut nb), 1),
            (0, w("zz-not-a-locale", &mut nb), 0),
            (4, enus, 0),
            (2, core::ptr::null(), 0),
        ] {
            check(create_substitution(factory, wanted, locale, ignore, &mut substitution) == 0, 49);
            check(!substitution.is_null(), 49);
            drop_com(substitution);
        }
        for wanted in [0u32, 1, 3, 4] {
            check(
                create_substitution(factory, wanted, core::ptr::null(), 0, &mut substitution)
                    == E_INVALIDARG,
                49,
            );
            check(create_substitution(factory, wanted, empty, 0, &mut substitution) == 0, 49);
            drop_com(substitution);
        }
        for wanted in [5u32, 9] {
            check(
                create_substitution(factory, wanted, enus, 0, &mut substitution) == E_INVALIDARG,
                49,
            );
        }

        // The GDI interop object's LOGFONT conversions (4, 5): how a
        // program that draws through GDI asks for the font it holds as
        // DirectWrite's. The height, the width, the escapement and the
        // orientation are zero -- a LOGFONT of a font and not of a size
        // -- the weight is the font's own, the character set is
        // DEFAULT_CHARSET (1) and the precision OUT_OUTLINE_PRECIS (8),
        // and the face name is the family's own name, whichever family
        // the collection answered with (so the name is read from the
        // collection here and not written down: gala's family is
        // Selawik where Windows' is Arial). A font of the system
        // collection is a system font.
        let get_interop: GetPtr = method(factory, 17);
        let mut interop: *mut c_void = core::ptr::null_mut();
        check(get_interop(factory, &mut interop) == 0 && !interop.is_null(), 50);
        let to_logfont: extern "system" fn(
            *mut c_void,
            *mut c_void,
            *mut LogFont,
            *mut i32,
        ) -> HResult = method(interop, 4);
        let face_to_logfont: extern "system" fn(*mut c_void, *mut c_void, *mut LogFont) -> HResult =
            method(interop, 5);
        let mut family_text = [0u16; 64];
        let family_string: extern "system" fn(*mut c_void, u32, *mut u16, u32) -> HResult =
            method(names, 8);
        check(family_string(names, 0, family_text.as_mut_ptr(), 64) == 0, 50);
        let mut lf = LogFont::EMPTY;
        let mut is_system = 0i32;
        check(to_logfont(interop, font, &mut lf, &mut is_system) == 0 && is_system != 0, 50);
        check(lf.height == 0 && lf.width == 0 && lf.escapement == 0 && lf.orientation == 0, 50);
        check(lf.weight == weight(font) as i32 && lf.italic == 0 && lf.underline == 0, 50);
        check(lf.strike_out == 0 && lf.charset == 1 && lf.out_precision == 8, 50);
        check(lf.clip_precision == 0 && lf.quality == 0 && lf.pitch_and_family == 0, 50);
        check(chars(&lf.face_name) == chars(&family_text), 50);
        check(
            to_logfont(interop, core::ptr::null_mut(), &mut lf, &mut is_system) == E_INVALIDARG,
            50,
        );

        // The same LOGFONT for that font's face (ConvertFontFaceToLOGFONT
        // reports no system flag: a face is one either way). The family
        // name and the weight are read from where step 50 read them,
        // because a refused conversion leaves its LOGFONT as it likes --
        // real DirectWrite clears the whole structure before it refuses.
        let mut face_lf = LogFont::EMPTY;
        check(face_to_logfont(interop, face, &mut face_lf) == 0, 51);
        check(face_lf.height == 0 && face_lf.weight == weight(font) as i32, 51);
        check(face_lf.charset == 1 && face_lf.out_precision == 8, 51);
        check(
            face_lf.pitch_and_family == 0 && chars(&face_lf.face_name) == chars(&family_text),
            51,
        );
        check(face_to_logfont(interop, core::ptr::null_mut(), &mut face_lf) == E_INVALIDARG, 51);
        drop_com(interop);

        // 14. The text layout (CreateTextLayout, 18;
        // CreateGdiCompatibleTextLayout, 19; CreateEllipsisTrimmingSign,
        // 20): what Word asks the factory for 414 times once its formats
        // are answered, and the busiest row of its whole startup. A
        // format of its own, so step 13's is left as step 13 set it.
        type MakeLayout = extern "system" fn(
            *mut c_void,
            *const u16,
            u32,
            *mut c_void,
            f32,
            f32,
            *mut *mut c_void,
        ) -> HResult;
        type MakeGdiLayout = extern "system" fn(
            *mut c_void,
            *const u16,
            u32,
            *mut c_void,
            f32,
            f32,
            f32,
            *const Matrix,
            i32,
            *mut *mut c_void,
        ) -> HResult;
        type SetRanged<T> = extern "system" fn(*mut c_void, T, Range) -> HResult;
        type GetRanged<T> = extern "system" fn(*mut c_void, u32, *mut T, *mut Range) -> HResult;
        type GetTextMetrics = extern "system" fn(*mut c_void, *mut TextMetrics) -> HResult;
        type GetLines = extern "system" fn(*mut c_void, *mut LineMetrics, u32, *mut u32) -> HResult;
        type GetClusters =
            extern "system" fn(*mut c_void, *mut ClusterMetrics, u32, *mut u32) -> HResult;
        type GetOverhang = extern "system" fn(*mut c_void, *mut Overhang) -> HResult;

        let make_layout: MakeLayout = method(factory, 18);
        let make_gdi_layout: MakeGdiLayout = method(factory, 19);
        let mut hb = [0u16; 64];
        let hello = w("Hello world", &mut hb);
        const HELLO: u32 = 11;
        let gdi_layout =
            |text: *const u16, len: u32, format: *mut c_void, cx: f32, cy: f32, ppd: f32| {
                let mut made: *mut c_void = core::ptr::null_mut();
                let r = make_gdi_layout(
                    factory,
                    text,
                    len,
                    format,
                    cx,
                    cy,
                    ppd,
                    core::ptr::null(),
                    0,
                    &mut made,
                );
                (r, made)
            };
        let plain_layout = |text: *const u16, len: u32, format: *mut c_void, cx: f32, cy: f32| {
            let mut made: *mut c_void = core::ptr::null_mut();
            let r = make_layout(factory, text, len, format, cx, cy, &mut made);
            (r, made)
        };

        let (r, tfmt) = make(arial, 400, 0, 5, 16.0, enus);
        check(r == 0 && !tfmt.is_null(), 52);

        // A layout is a string, a format and a box. The string may be
        // empty but its pointer may not be NULL; the box may be empty
        // but neither side may be negative; and the pixels per DIP of a
        // GDI-compatible layout are taken as they come -- zero and a
        // negative are both S_OK (measured), as are a transform, a zero
        // transform and use_gdi_natural.
        let (r, layout) = gdi_layout(hello, HELLO, tfmt, 200.0, 100.0, 1.0);
        check(r == 0 && !layout.is_null(), 52);
        let (r, plain) = plain_layout(hello, HELLO, tfmt, 200.0, 100.0);
        check(r == 0 && !plain.is_null(), 52);
        check(
            gdi_layout(hello, HELLO, core::ptr::null_mut(), 200.0, 100.0, 1.0).0 == E_INVALIDARG,
            52,
        );
        check(
            plain_layout(hello, HELLO, core::ptr::null_mut(), 200.0, 100.0).0 == E_INVALIDARG,
            52,
        );
        check(gdi_layout(core::ptr::null(), HELLO, tfmt, 200.0, 100.0, 1.0).0 == E_INVALIDARG, 52);
        check(gdi_layout(core::ptr::null(), 0, tfmt, 200.0, 100.0, 1.0).0 == E_INVALIDARG, 52);
        check(plain_layout(core::ptr::null(), 0, tfmt, 200.0, 100.0).0 == E_INVALIDARG, 52);
        check(gdi_layout(hello, HELLO, tfmt, -1.0, 100.0, 1.0).0 == E_INVALIDARG, 52);
        check(gdi_layout(hello, HELLO, tfmt, 200.0, -1.0, 1.0).0 == E_INVALIDARG, 52);
        check(plain_layout(hello, HELLO, tfmt, -1.0, 100.0).0 == E_INVALIDARG, 52);
        for (r, made) in [
            gdi_layout(hello, 0, tfmt, 200.0, 100.0, 1.0),
            gdi_layout(hello, HELLO, tfmt, 0.0, 0.0, 1.0),
            gdi_layout(hello, HELLO, tfmt, 200.0, 100.0, 0.0),
            gdi_layout(hello, HELLO, tfmt, 200.0, 100.0, -1.0),
        ] {
            check(r == 0 && !made.is_null(), 52);
            drop_com(made);
        }
        let identity = Matrix { m11: 1.0, m12: 0.0, m21: 0.0, m22: 1.0, dx: 0.0, dy: 0.0 };
        let flat = Matrix { m11: 0.0, m12: 0.0, m21: 0.0, m22: 0.0, dx: 0.0, dy: 0.0 };
        for (transform, natural) in [
            (core::ptr::null(), 1i32),
            (&identity as *const Matrix, 0),
            (&flat as *const Matrix, 0),
        ] {
            let mut made: *mut c_void = core::ptr::null_mut();
            let r = make_gdi_layout(
                factory, hello, HELLO, tfmt, 200.0, 100.0, 2.0, transform, natural, &mut made,
            );
            check(r == 0 && !made.is_null(), 52);
            drop_com(made);
        }

        // Its interfaces: itself for IDWriteTextLayout and for IUnknown,
        // a text format (Windows answers that one with an object of its
        // own where gala answers itself, so only the answer is
        // asserted), and the refusal every QueryInterface owes. The
        // layout's own revisions are step 11's divergence again.
        let layout_qi: extern "system" fn(*mut c_void, *const Iid, *mut *mut c_void) -> HResult =
            method(layout, 0);
        let layout_add_ref: GetU32 = method(layout, 1);
        let layout_release: GetU32 = method(layout, 2);
        let mut asked: *mut c_void = core::ptr::null_mut();
        check(layout_qi(layout, &IID_TEXT_LAYOUT, &mut asked) == 0 && asked == layout, 53);
        drop_com(asked);
        check(layout_qi(layout, &IID_UNKNOWN, &mut asked) == 0 && !asked.is_null(), 53);
        drop_com(asked);
        check(layout_qi(layout, &IID_TEXT_FORMAT, &mut asked) == 0 && !asked.is_null(), 53);
        drop_com(asked);
        asked = layout;
        check(
            layout_qi(layout, &IID_NOT_AN_INTERFACE, &mut asked) == E_NOINTERFACE
                && asked.is_null(),
            53,
        );
        for iid in IID_TEXT_LAYOUT_REVISIONS {
            asked = layout;
            let hr = layout_qi(layout, &iid, &mut asked);
            check((hr == 0 && !asked.is_null()) || (hr == E_NOINTERFACE && asked.is_null()), 53);
            if hr == 0 {
                drop_com(asked);
            }
        }
        check(layout_add_ref(layout) == 2 && layout_release(layout) == 1, 53);

        // A layout is a text format as well, answering the format's own
        // 28 slots with the values it was made with -- and with a copy
        // of them: what the format is told afterwards does not reach the
        // layout (measured).
        let layout_weight: GetU32 = method(layout, 22);
        let layout_style: GetU32 = method(layout, 23);
        let layout_stretch: GetU32 = method(layout, 24);
        let layout_size: GetF32 = method(layout, 25);
        let layout_family_len: GetU32 = method(layout, 20);
        let layout_family: ReadString = method(layout, 21);
        let layout_locale_len: GetU32 = method(layout, 26);
        check(layout_weight(layout) == 400 && layout_style(layout) == 0, 54);
        check(layout_stretch(layout) == 5 && layout_size(layout) == 16.0, 54);
        check(layout_family_len(layout) == 5 && layout_locale_len(layout) == 5, 54);
        check(layout_family(layout, read.as_mut_ptr(), 6) == 0 && is_text(&read, "Arial"), 54);
        let layout_alignment: GetU32 = method(layout, 11);
        let layout_paragraph: GetU32 = method(layout, 12);
        let layout_wrapping: GetU32 = method(layout, 13);
        let layout_reading: GetU32 = method(layout, 14);
        let layout_flow: GetU32 = method(layout, 15);
        let layout_tab: GetF32 = method(layout, 16);
        check(layout_alignment(layout) == 0 && layout_paragraph(layout) == 0, 54);
        check(layout_wrapping(layout) == 0 && layout_reading(layout) == 0, 54);
        check(layout_flow(layout) == 0 && layout_tab(layout) == 64.0, 54);
        let format_wrapping: SetU32 = method(tfmt, 5);
        check(format_wrapping(tfmt, 1) == 0 && layout_wrapping(layout) == 0, 54);
        check(format_wrapping(tfmt, 0) == 0, 54);

        // Its measurements. The text's box sits at the layout box's
        // origin, no wider than its one line, and the layout's own box
        // is reported back with it; plain left-to-right text is one
        // bidi level deep.
        let layout_metrics: GetTextMetrics = method(layout, 60);
        let mut tm = TextMetrics::ZERO;
        check(layout_metrics(layout, &mut tm) == 0, 55);
        check(near(tm.left, 0.0) && near(tm.top, 0.0) && tm.width > 0.0, 55);
        check(near(tm.width_with_trailing, tm.width) && tm.width < 200.0, 55);
        check(near(tm.layout_width, 200.0) && near(tm.layout_height, 100.0), 55);
        check(tm.max_bidi_depth == 1 && tm.line_count == 1, 55);

        // The lines, which is where a GDI-compatible layout's rounding
        // shows (step 68 says against what). A buffer that cannot hold
        // them all is E_NOT_SUFFICIENT_BUFFER with the count written, as
        // every counted getter here is.
        let layout_lines: GetLines = method(layout, 59);
        let mut count = 0u32;
        check(
            layout_lines(layout, core::ptr::null_mut(), 0, &mut count) == E_NOT_SUFFICIENT_BUFFER,
            56,
        );
        check(count == 1, 56);
        let mut lines = [LineMetrics::ZERO; 8];
        check(layout_lines(layout, lines.as_mut_ptr(), 1, &mut count) == 0 && count == 1, 56);
        let line = lines[0];
        check(line.length == HELLO && line.trailing_whitespace == 0, 56);
        check(line.newline_length == 0 && line.is_trimmed == 0, 56);
        check(line.height > 0.0 && line.baseline > 0.0 && line.baseline < line.height, 56);
        check(near(tm.height, line.height), 56);

        // The clusters: one per character here, each a whole number of
        // pixels wide (that is what GDI-compatible means at one pixel
        // per DIP), together as wide as the text, the space the only
        // whitespace and -- with the end of the text -- the only place a
        // line may break.
        let layout_clusters: GetClusters = method(layout, 62);
        count = 0;
        check(
            layout_clusters(layout, core::ptr::null_mut(), 0, &mut count)
                == E_NOT_SUFFICIENT_BUFFER,
            57,
        );
        check(count == HELLO, 57);
        let mut clusters = [ClusterMetrics::ZERO; 16];
        check(layout_clusters(layout, clusters.as_mut_ptr(), HELLO, &mut count) == 0, 57);
        check(count == HELLO, 57);
        let mut sum = 0.0f32;
        for (i, c) in clusters[..HELLO as usize].iter().enumerate() {
            check(c.length == 1 && c.width > 0.0, 57);
            check(c.width == (c.width as i32) as f32, 57);
            check(c.whitespace() == (i == 5), 57);
            check(!c.newline() && !c.soft_hyphen() && !c.right_to_left(), 57);
            check(c.can_wrap_after() == (i == 5 || i == 10), 57);
            sum += c.width;
        }
        check(near(sum, tm.width), 57);

        // The narrowest the text can be laid out: the widest run that
        // cannot be broken, its trailing spaces not counted. Derived
        // here from the layout's own cluster widths, so the check holds
        // whatever the font is.
        let min_width: extern "system" fn(*mut c_void, *mut f32) -> HResult = method(layout, 63);
        let mut narrowest = 0.0f32;
        check(min_width(layout, &mut narrowest) == 0, 58);
        let (mut widest, mut run, mut spaces) = (0.0f32, 0.0f32, 0.0f32);
        for c in clusters[..HELLO as usize].iter() {
            if c.whitespace() {
                spaces += c.width;
            } else {
                run += spaces + c.width;
                spaces = 0.0;
            }
            if c.can_wrap_after() {
                if run > widest {
                    widest = run;
                }
                run = 0.0;
                spaces = 0.0;
            }
        }
        check(run <= widest && near(narrowest, widest), 58);

        // The ink against the box: positive is ink outside it, and text
        // this short is inside on every side. The right and the bottom
        // are the text's own width and baseline measured from the box's
        // far edges, which is what makes them so large a negative.
        let layout_overhang: GetOverhang = method(layout, 61);
        let mut overhang = Overhang::ZERO;
        check(layout_overhang(layout, &mut overhang) == 0, 59);
        check(overhang.left <= 0.0 && overhang.left >= -2.0, 59);
        check(overhang.top <= 0.0 && overhang.top >= -line.height, 59);
        check(overhang.right <= 0.0 && overhang.right >= tm.width - tm.layout_width - 2.0, 59);
        check(
            overhang.bottom <= 0.0 && overhang.bottom >= line.baseline - tm.layout_height - 2.0,
            59,
        );

        // The box can be changed after the fact, and a negative side is
        // refused without disturbing the one it has.
        let max_width: GetF32 = method(layout, 42);
        let max_height: GetF32 = method(layout, 43);
        let set_max_width: SetF32 = method(layout, 28);
        let set_max_height: SetF32 = method(layout, 29);
        check(max_width(layout) == 200.0 && max_height(layout) == 100.0, 60);
        check(set_max_width(layout, 300.0) == 0 && max_width(layout) == 300.0, 60);
        check(set_max_height(layout, 50.0) == 0 && max_height(layout) == 50.0, 60);
        let mut again_tm = TextMetrics::ZERO;
        check(layout_metrics(layout, &mut again_tm) == 0, 60);
        check(near(again_tm.layout_width, 300.0) && near(again_tm.layout_height, 50.0), 60);
        check(near(again_tm.width, tm.width) && again_tm.line_count == 1, 60);
        check(set_max_width(layout, -1.0) == E_INVALIDARG, 60);
        check(set_max_height(layout, -1.0) == E_INVALIDARG, 60);
        check(max_width(layout) == 300.0 && max_height(layout) == 50.0, 60);
        check(set_max_width(layout, 200.0) == 0 && set_max_height(layout, 100.0) == 0, 60);

        // Wrapping is greedy at the opportunities the clusters name: a
        // box as wide as "Hello " and no wider leaves "world" for a
        // second line, which keeps the trailing space on the first.
        // The width comes from the layout's own measurements, so the
        // check does not depend on the font either.
        let through_space: f32 = clusters[..6].iter().map(|c| c.width).sum();
        let (r, wrapped) = gdi_layout(hello, HELLO, tfmt, through_space, 100.0, 1.0);
        check(r == 0 && !wrapped.is_null(), 61);
        let wrapped_metrics: GetTextMetrics = method(wrapped, 60);
        let wrapped_lines: GetLines = method(wrapped, 59);
        let mut wrapped_tm = TextMetrics::ZERO;
        check(wrapped_metrics(wrapped, &mut wrapped_tm) == 0 && wrapped_tm.line_count == 2, 61);
        count = 0;
        check(wrapped_lines(wrapped, lines.as_mut_ptr(), 8, &mut count) == 0 && count == 2, 61);
        check(lines[0].length == 6 && lines[0].trailing_whitespace == 1, 61);
        check(lines[1].length == 5 && lines[1].trailing_whitespace == 0, 61);
        check(near(wrapped_tm.height, lines[0].height + lines[1].height), 61);
        check(wrapped_tm.width > 0.0 && wrapped_tm.width <= through_space + 0.01, 61);
        drop_com(wrapped);

        // Narrower than a single character there is nowhere to break, so
        // the layout breaks between characters rather than overflow (an
        // emergency break): every line holds something and together they
        // hold the whole string.
        let (r, tight) = gdi_layout(hello, HELLO, tfmt, clusters[0].width, 400.0, 1.0);
        check(r == 0 && !tight.is_null(), 61);
        let tight_lines: GetLines = method(tight, 59);
        count = 0;
        check(
            tight_lines(tight, core::ptr::null_mut(), 0, &mut count) == E_NOT_SUFFICIENT_BUFFER,
            61,
        );
        check((2..=HELLO).contains(&count), 61);
        let mut many = [LineMetrics::ZERO; 16];
        check(tight_lines(tight, many.as_mut_ptr(), 16, &mut count) == 0, 61);
        let mut characters = 0u32;
        for l in many[..count as usize].iter() {
            check(l.length > 0, 61);
            characters += l.length;
        }
        check(characters == HELLO, 61);
        drop_com(tight);

        // Trailing whitespace is in the line and in the text's length,
        // but not in the width the text is measured at -- that is what
        // the second width is for.
        let mut sb = [0u16; 64];
        let spaced = w("Hello world   ", &mut sb);
        let (r, trailing_layout) = gdi_layout(spaced, 14, tfmt, 200.0, 100.0, 1.0);
        check(r == 0 && !trailing_layout.is_null(), 62);
        let trailing_metrics: GetTextMetrics = method(trailing_layout, 60);
        let trailing_lines: GetLines = method(trailing_layout, 59);
        let mut trailing_tm = TextMetrics::ZERO;
        check(trailing_metrics(trailing_layout, &mut trailing_tm) == 0, 62);
        check(near(trailing_tm.width, tm.width), 62);
        check(near(trailing_tm.width_with_trailing, tm.width + 3.0 * clusters[5].width), 62);
        count = 0;
        check(trailing_lines(trailing_layout, lines.as_mut_ptr(), 8, &mut count) == 0, 62);
        check(count == 1 && lines[0].length == 14 && lines[0].trailing_whitespace == 3, 62);
        drop_com(trailing_layout);

        // A line break ends a line and belongs to it: its characters
        // count in the line's length, its trailing whitespace and its
        // newline length, and a CRLF counts as the two it is. A break
        // takes no width and is whitespace; of a CRLF only the LF is the
        // newline and the place the line may end.
        let mut pb = [0u16; 64];
        let paragraphs = w("a\r\nb\nc", &mut pb);
        let (r, broken) = gdi_layout(paragraphs, 6, tfmt, 200.0, 400.0, 1.0);
        check(r == 0 && !broken.is_null(), 63);
        let broken_metrics: GetTextMetrics = method(broken, 60);
        let broken_lines: GetLines = method(broken, 59);
        let broken_clusters: GetClusters = method(broken, 62);
        let mut broken_tm = TextMetrics::ZERO;
        check(broken_metrics(broken, &mut broken_tm) == 0 && broken_tm.line_count == 3, 63);
        count = 0;
        check(broken_lines(broken, lines.as_mut_ptr(), 8, &mut count) == 0 && count == 3, 63);
        check(lines[0].length == 3 && lines[0].trailing_whitespace == 2, 63);
        check(lines[0].newline_length == 2 && lines[1].length == 2, 63);
        check(lines[1].trailing_whitespace == 1 && lines[1].newline_length == 1, 63);
        check(lines[2].length == 1 && lines[2].trailing_whitespace == 0, 63);
        check(lines[2].newline_length == 0, 63);
        check(near(broken_tm.height, lines[0].height + lines[1].height + lines[2].height), 63);
        let mut breaks = [ClusterMetrics::ZERO; 8];
        count = 0;
        check(broken_clusters(broken, breaks.as_mut_ptr(), 8, &mut count) == 0 && count == 6, 63);
        for i in [1usize, 2, 4] {
            check(breaks[i].width == 0.0 && breaks[i].whitespace(), 63);
        }
        check(!breaks[1].newline() && breaks[2].newline() && breaks[4].newline(), 63);
        check(!breaks[1].can_wrap_after() && breaks[2].can_wrap_after(), 63);
        check(breaks[4].can_wrap_after(), 63);
        for i in [0usize, 3, 5] {
            check(breaks[i].width > 0.0 && !breaks[i].whitespace(), 63);
            check(!breaks[i].newline(), 63);
        }
        drop_com(broken);

        // The ranged attributes, which is how a layout differs from a
        // format: every value a format holds for the whole text a layout
        // holds over a range of it, and reports with the run it found it
        // in. What no range covers is the format's own value over a run
        // that reaches the end of everything (measured: to UINT32_MAX).
        // A position past the text, a range wholly past it, an empty
        // range and a NULL run are all taken, and the values a format
        // refuses a layout refuses as well.
        let set_weight: SetRanged<u32> = method(layout, 32);
        let set_style: SetRanged<u32> = method(layout, 33);
        let set_stretch: SetRanged<u32> = method(layout, 34);
        let set_size: SetRanged<f32> = method(layout, 35);
        let set_underline: SetRanged<i32> = method(layout, 36);
        let set_strikethrough: SetRanged<i32> = method(layout, 37);
        let get_weight: GetRanged<u32> = method(layout, 47);
        let get_style: GetRanged<u32> = method(layout, 48);
        let get_stretch: GetRanged<u32> = method(layout, 49);
        let get_size: GetRanged<f32> = method(layout, 50);
        let get_underline: GetRanged<i32> = method(layout, 51);
        let get_strikethrough: GetRanged<i32> = method(layout, 52);
        let first5 = Range { start: 0, length: 5 };
        let (mut value, mut run_of) = (0u32, Range::ZERO);
        check(set_weight(layout, 700, first5) == 0, 64);
        check(get_weight(layout, 0, &mut value, &mut run_of) == 0, 64);
        check(value == 700 && run_of.start == 0 && run_of.length == 5, 64);
        check(get_weight(layout, 7, &mut value, &mut run_of) == 0, 64);
        check(value == 400 && run_of.start == 5 && run_of.length == u32::MAX - 5, 64);
        check(get_weight(layout, 1000, &mut value, &mut run_of) == 0 && value == 400, 64);
        check(get_weight(layout, 0, &mut value, core::ptr::null_mut()) == 0 && value == 700, 64);
        check(set_weight(layout, 300, Range { start: 50, length: 10 }) == 0, 64);
        check(set_weight(layout, 300, Range { start: 3, length: 0 }) == 0, 64);
        check(get_weight(layout, 3, &mut value, &mut run_of) == 0 && value == 700, 64);
        check(set_weight(layout, 0, first5) == E_INVALIDARG, 64);
        check(set_weight(layout, 1000, first5) == E_INVALIDARG, 64);
        check(
            set_style(layout, 2, first5) == 0 && set_style(layout, 3, first5) == E_INVALIDARG,
            64,
        );
        check(get_style(layout, 0, &mut value, &mut run_of) == 0 && value == 2, 64);
        check(get_style(layout, 9, &mut value, &mut run_of) == 0 && value == 0, 64);
        check(set_stretch(layout, 9, first5) == 0, 64);
        check(set_stretch(layout, 0, first5) == E_INVALIDARG, 64);
        check(set_stretch(layout, 10, first5) == E_INVALIDARG, 64);
        check(get_stretch(layout, 0, &mut value, &mut run_of) == 0 && value == 9, 64);
        let mut size = 0.0f32;
        check(set_size(layout, 24.0, first5) == 0, 64);
        check(set_size(layout, 0.0, first5) == E_INVALIDARG, 64);
        check(set_size(layout, -1.0, first5) == E_INVALIDARG, 64);
        check(get_size(layout, 0, &mut size, &mut run_of) == 0 && size == 24.0, 64);
        check(get_size(layout, 9, &mut size, &mut run_of) == 0 && size == 16.0, 64);
        let mut flag = 0i32;
        check(
            set_underline(layout, 1, first5) == 0 && set_strikethrough(layout, 1, first5) == 0,
            64,
        );
        check(get_underline(layout, 0, &mut flag, &mut run_of) == 0 && flag != 0, 64);
        check(get_underline(layout, 9, &mut flag, &mut run_of) == 0 && flag == 0, 64);
        check(get_strikethrough(layout, 0, &mut flag, &mut run_of) == 0 && flag != 0, 64);

        // A family name over a range is kept as verbatim as a format's
        // (step 45): a family no collection has is taken and read back
        // as it was given, and what the range does not cover is still
        // the format's own. A locale name is held to BCP-47's shape, as
        // a format's is.
        let set_family: SetRanged<*const u16> = method(layout, 31);
        let set_locale: SetRanged<*const u16> = method(layout, 41);
        let family_len_at: GetRanged<u32> = method(layout, 45);
        let family_at: extern "system" fn(*mut c_void, u32, *mut u16, u32, *mut Range) -> HResult =
            method(layout, 46);
        let mut mb = [0u16; 64];
        let made_up = w("No Such Family Here!!", &mut mb);
        check(set_family(layout, made_up, first5) == 0, 64);
        check(set_family(layout, core::ptr::null(), first5) == E_INVALIDARG, 64);
        let mut family_len = 0u32;
        check(family_len_at(layout, 0, &mut family_len, &mut run_of) == 0, 64);
        check(family_len == 21 && run_of.start == 0 && run_of.length == 5, 64);
        check(
            family_at(layout, 0, read.as_mut_ptr(), 1, &mut run_of) == E_NOT_SUFFICIENT_BUFFER,
            64,
        );
        check(family_at(layout, 0, read.as_mut_ptr(), 22, &mut run_of) == 0, 64);
        check(is_text(&read, "No Such Family Here!!"), 64);
        check(family_at(layout, 9, read.as_mut_ptr(), 22, &mut run_of) == 0, 64);
        check(is_text(&read, "Arial"), 64);
        check(set_locale(layout, w("sv-se", &mut nb), first5) == 0, 64);
        check(set_locale(layout, w("not a tag!", &mut nb), first5) == E_INVALIDARG, 64);
        check(set_locale(layout, core::ptr::null(), first5) == E_INVALIDARG, 64);

        // A drawing effect, an inline object and a typography are held
        // as they are given, NULL included -- the layout does not look
        // at them (what draws does, and nothing draws here).
        let set_effect: SetRanged<*mut c_void> = method(layout, 38);
        let set_inline: SetRanged<*mut c_void> = method(layout, 39);
        let set_typography: SetRanged<*mut c_void> = method(layout, 40);
        let set_collection: SetRanged<*mut c_void> = method(layout, 30);
        let get_effect: GetRanged<*mut c_void> = method(layout, 53);
        let get_inline: GetRanged<*mut c_void> = method(layout, 54);
        let get_typography: GetRanged<*mut c_void> = method(layout, 55);
        check(set_effect(layout, core::ptr::null_mut(), first5) == 0, 64);
        check(set_inline(layout, core::ptr::null_mut(), first5) == 0, 64);
        check(set_typography(layout, core::ptr::null_mut(), first5) == 0, 64);
        check(set_collection(layout, core::ptr::null_mut(), first5) == 0, 64);
        let mut object: *mut c_void = layout;
        check(get_effect(layout, 0, &mut object, &mut run_of) == 0 && object.is_null(), 64);
        object = layout;
        check(get_inline(layout, 0, &mut object, &mut run_of) == 0 && object.is_null(), 64);
        object = layout;
        check(get_typography(layout, 0, &mut object, &mut run_of) == 0 && object.is_null(), 64);
        // The ranges are measured with: a larger size over part of the
        // text makes the text wider and its line taller, in one line
        // still.
        check(layout_metrics(layout, &mut again_tm) == 0 && again_tm.line_count == 1, 64);
        check(again_tm.width > tm.width && again_tm.height > tm.height, 64);

        // Hit testing, both ways, on a layout none of that touched. A
        // point inside the first character is that character, not a
        // trailing hit; the first position starts at the origin and is
        // its own cluster wide and the line tall; and a range of five
        // characters is one box of their widths.
        let (r, hit) = gdi_layout(hello, HELLO, tfmt, 200.0, 100.0, 1.0);
        check(r == 0 && !hit.is_null(), 65);
        let hit_point: extern "system" fn(
            *mut c_void,
            f32,
            f32,
            *mut i32,
            *mut i32,
            *mut HitTestMetrics,
        ) -> HResult = method(hit, 64);
        let hit_position: extern "system" fn(
            *mut c_void,
            u32,
            i32,
            *mut f32,
            *mut f32,
            *mut HitTestMetrics,
        ) -> HResult = method(hit, 65);
        let hit_range: extern "system" fn(
            *mut c_void,
            u32,
            u32,
            f32,
            f32,
            *mut HitTestMetrics,
            u32,
            *mut u32,
        ) -> HResult = method(hit, 66);
        let (mut trailing, mut inside) = (1i32, 0i32);
        let mut where_at = HitTestMetrics::ZERO;
        check(hit_point(hit, 1.0, 1.0, &mut trailing, &mut inside, &mut where_at) == 0, 65);
        check(trailing == 0 && inside != 0, 65);
        check(where_at.text_position == 0 && where_at.length == 1, 65);
        check(where_at.is_text != 0 && where_at.is_trimmed == 0 && where_at.bidi_level == 0, 65);
        let (mut x, mut y) = (1.0f32, 1.0f32);
        check(hit_position(hit, 0, 0, &mut x, &mut y, &mut where_at) == 0, 65);
        check(near(x, 0.0) && near(y, 0.0), 65);
        check(near(where_at.width, clusters[0].width), 65);
        check(near(where_at.height, line.height) && where_at.length == 1, 65);
        count = 0;
        check(
            hit_range(hit, 0, 5, 0.0, 0.0, core::ptr::null_mut(), 0, &mut count)
                == E_NOT_SUFFICIENT_BUFFER,
            65,
        );
        check(count == 1, 65);
        let mut boxes = [HitTestMetrics::ZERO; 4];
        check(hit_range(hit, 0, 5, 0.0, 0.0, boxes.as_mut_ptr(), 4, &mut count) == 0, 65);
        check(count == 1, 65);
        let first_five: f32 = clusters[..5].iter().map(|c| c.width).sum();
        check(boxes[0].text_position == 0 && boxes[0].length == 5, 65);
        check(near(boxes[0].left, 0.0) && near(boxes[0].top, 0.0), 65);
        check(near(boxes[0].width, first_five) && near(boxes[0].height, line.height), 65);
        check(boxes[0].is_text != 0, 65);
        drop_com(hit);

        // No text at all is still a line: a layout of nothing has no
        // width, the line's height, one line and no clusters -- and no
        // clusters is S_OK and not a buffer too small (measured).
        let (r, nothing) = gdi_layout(hello, 0, tfmt, 200.0, 100.0, 1.0);
        check(r == 0 && !nothing.is_null(), 66);
        let nothing_metrics: GetTextMetrics = method(nothing, 60);
        let nothing_lines: GetLines = method(nothing, 59);
        let nothing_clusters: GetClusters = method(nothing, 62);
        let mut nothing_tm = TextMetrics::ZERO;
        check(nothing_metrics(nothing, &mut nothing_tm) == 0, 66);
        check(nothing_tm.width == 0.0 && near(nothing_tm.height, line.height), 66);
        check(nothing_tm.line_count == 1, 66);
        count = 0;
        check(nothing_lines(nothing, lines.as_mut_ptr(), 8, &mut count) == 0 && count == 1, 66);
        check(lines[0].length == 0 && near(lines[0].height, line.height), 66);
        count = 7;
        check(nothing_clusters(nothing, core::ptr::null_mut(), 0, &mut count) == 0, 66);
        check(count == 0, 66);
        drop_com(nothing);

        // The ellipsis trimming sign, which Word asks for beside its
        // layouts: an inline object of a format's own font, as wide as
        // the ellipsis it draws and of no height of its own -- it sits
        // on the baseline it is given. It breaks neither way (NEUTRAL)
        // and answers IDWriteInlineObject. A NULL format is
        // E_INVALIDARG.
        type MakeSign = extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> HResult;
        let make_sign: MakeSign = method(factory, 20);
        let mut sign: *mut c_void = core::ptr::null_mut();
        check(make_sign(factory, tfmt, &mut sign) == 0 && !sign.is_null(), 67);
        let mut refused: *mut c_void = core::ptr::null_mut();
        check(make_sign(factory, core::ptr::null_mut(), &mut refused) == E_INVALIDARG, 67);
        let sign_qi: extern "system" fn(*mut c_void, *const Iid, *mut *mut c_void) -> HResult =
            method(sign, 0);
        asked = core::ptr::null_mut();
        check(sign_qi(sign, &IID_INLINE_OBJECT, &mut asked) == 0 && !asked.is_null(), 67);
        drop_com(asked);
        let sign_metrics: extern "system" fn(*mut c_void, *mut InlineMetrics) -> HResult =
            method(sign, 4);
        let sign_overhang: GetOverhang = method(sign, 5);
        let sign_breaks: extern "system" fn(*mut c_void, *mut u32, *mut u32) -> HResult =
            method(sign, 6);
        let mut sign_box = InlineMetrics::ZERO;
        check(sign_metrics(sign, &mut sign_box) == 0, 67);
        check(sign_box.width > 0.0 && sign_box.height == 0.0, 67);
        check(sign_box.baseline == 0.0 && sign_box.supports_sideways == 0, 67);
        let mut sign_ink = Overhang::ZERO;
        check(sign_overhang(sign, &mut sign_ink) == 0, 67);
        let (mut before, mut after) = (9u32, 9u32);
        check(sign_breaks(sign, &mut before, &mut after) == 0 && before == 0 && after == 0, 67);
        drop_com(sign);

        // What "GDI-compatible" means, in numbers: the layout measures
        // the text as GDI measures it. The same family at the same pixel
        // size through a memory device context gives the line's height
        // (tmHeight), its baseline (tmAscent) and the layout's width
        // (GetTextExtentPoint32W), measured over four sizes and three
        // families on Windows. It is the only comparison there is:
        // GDI's own numbers come from the face's hinted metrics and are
        // not the design values scaled, so a formula fitted to those
        // would hold on neither system.
        let dc = CreateCompatibleDC(core::ptr::null_mut());
        check(!dc.is_null(), 68);
        for em in [10.0f32, 12.0, 16.0, 24.0] {
            let (r, sized) = make(arial, 400, 0, 5, em, enus);
            check(r == 0 && !sized.is_null(), 68);
            let (r, measured) = gdi_layout(hello, HELLO, sized, 1000.0, 1000.0, 1.0);
            check(r == 0 && !measured.is_null(), 68);
            let (height, ascent, width) =
                gdi_text(dc, arial, -(em as i32), hello, HELLO as i32, 68);
            let sized_lines: GetLines = method(measured, 59);
            let sized_metrics: GetTextMetrics = method(measured, 60);
            count = 0;
            check(sized_lines(measured, lines.as_mut_ptr(), 8, &mut count) == 0 && count == 1, 68);
            let mut sized_tm = TextMetrics::ZERO;
            check(sized_metrics(measured, &mut sized_tm) == 0, 68);
            check(near(lines[0].height, height as f32), 68);
            check(near(lines[0].baseline, ascent as f32), 68);
            check(near(sized_tm.width, width as f32), 68);
            drop_com(measured);
            drop_com(sized);
        }

        // The pixels per DIP are a scale on that pixel grid and nothing
        // else: an em of e at p pixels per DIP measures what GDI
        // measures at e * p pixels, reported in DIPs -- divided by p
        // again. Measured at 1.5 and 2, where the numbers are those of
        // 24 and 32 px.
        for ppd in [1.5f32, 2.0] {
            let (r, scaled) = gdi_layout(hello, HELLO, tfmt, 1000.0, 1000.0, ppd);
            check(r == 0 && !scaled.is_null(), 69);
            let pixels = (16.0 * ppd + 0.5) as i32;
            let (height, ascent, width) = gdi_text(dc, arial, -pixels, hello, HELLO as i32, 69);
            let scaled_lines: GetLines = method(scaled, 59);
            let scaled_metrics: GetTextMetrics = method(scaled, 60);
            count = 0;
            check(scaled_lines(scaled, lines.as_mut_ptr(), 8, &mut count) == 0 && count == 1, 69);
            let mut scaled_tm = TextMetrics::ZERO;
            check(scaled_metrics(scaled, &mut scaled_tm) == 0, 69);
            check(near(lines[0].height * ppd, height as f32), 69);
            check(near(lines[0].baseline * ppd, ascent as f32), 69);
            check(near(scaled_tm.width * ppd, width as f32), 69);
            drop_com(scaled);
        }
        check(DeleteDC(dc) != 0, 69);

        // And the plain layout is the unrounded one: its line is the
        // face's own design metrics scaled by the em size, with the gap
        // above the ascent belonging to the baseline. The numbers are
        // step 4's, read from the font and not written down here.
        let upem = f32::from(metrics[0]);
        let ascender = f32::from(metrics[1]);
        let descender = f32::from(metrics[2]);
        let gap = f32::from(metrics[3] as i16);
        let plain_lines: GetLines = method(plain, 59);
        let plain_metrics: GetTextMetrics = method(plain, 60);
        count = 0;
        check(plain_lines(plain, lines.as_mut_ptr(), 8, &mut count) == 0 && count == 1, 69);
        let mut plain_tm = TextMetrics::ZERO;
        check(plain_metrics(plain, &mut plain_tm) == 0, 69);
        check(near(lines[0].height, (ascender + descender + gap) * 16.0 / upem), 69);
        check(near(lines[0].baseline, (ascender + gap) * 16.0 / upem), 69);
        check(near(plain_tm.height, lines[0].height), 69);
        drop_com(plain);
        drop_com(layout);
        drop_com(tfmt);

        // Release what we were given (each Release is slot 2).
        for o in
            [again, stream, local, loader, file, face, font, names, family, collection, factory]
        {
            let release: GetU32 = method(o, 2);
            release(o);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    tests();
    out(b"dwrite_test: ok\r\n");
    // SAFETY: ends the process.
    unsafe { ExitProcess(0) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
