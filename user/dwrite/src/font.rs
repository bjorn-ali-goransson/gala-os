//! The system font collection: one family (Selawik), its four fonts, their
//! faces, files, the local file loader, file streams and localized
//! strings, all static (see the crate documentation).

use crate::{
    E_INVALIDARG, E_NOTIMPL, E_POINTER, HResult, Iid, S_OK, VTable, guid, missing, query, vt,
};
use core::ffi::c_void;
use core::sync::atomic::{AtomicPtr, AtomicU8, AtomicUsize, Ordering};

const IID_COLLECTION: Iid =
    guid(0xa84cee02, 0x3eea, 0x4eee, [0xa8, 0x27, 0x87, 0xc1, 0xa0, 0x2a, 0x0f, 0xcc]);
const IID_FAMILY: Iid =
    guid(0xda20d8ef, 0x812a, 0x4c43, [0x98, 0x02, 0x62, 0xec, 0x4a, 0xbd, 0x7a, 0xdd]);
const IID_FONT_LIST: Iid =
    guid(0x1a0d8438, 0x1d97, 0x4ec1, [0xae, 0xf9, 0xa2, 0xfb, 0x86, 0xed, 0x6a, 0xcb]);
const IID_FONT: Iid =
    guid(0xacd16696, 0x8c14, 0x4f5d, [0x87, 0x7e, 0xfe, 0x3f, 0xc1, 0xd3, 0x27, 0x37]);
const IID_STRINGS: Iid =
    guid(0x08256209, 0x099a, 0x4b34, [0xb8, 0x6d, 0xc2, 0x2b, 0x11, 0x0e, 0x77, 0x71]);
const IID_FACE: Iid =
    guid(0x5f49804d, 0x7024, 0x4d43, [0xbf, 0xa9, 0xd2, 0x59, 0x84, 0xf5, 0x38, 0x49]);
const IID_FILE: Iid =
    guid(0x739d886a, 0xcef5, 0x47dc, [0x87, 0x69, 0x1a, 0x8b, 0x41, 0xbe, 0xbb, 0xb0]);
const IID_FILE_LOADER: Iid =
    guid(0x727cad4e, 0xd6af, 0x4c9e, [0x8a, 0x08, 0xd6, 0x95, 0xb1, 0x1c, 0xaa, 0x49]);
const IID_LOCAL_LOADER: Iid =
    guid(0xb2d9f3ec, 0xc9fe, 0x4a11, [0xa2, 0xec, 0xd8, 0x62, 0x08, 0xf7, 0xc0, 0xa2]);
const IID_STREAM: Iid =
    guid(0x6d4865fe, 0x0ab8, 0x4d91, [0x8f, 0x62, 0x5d, 0xd6, 0xbe, 0x34, 0xa3, 0xe0]);

/// DWRITE_E_NOFONT.
const DWRITE_E_NOFONT: HResult = 0x8898_5002_u32 as i32;
/// E_NOT_SUFFICIENT_BUFFER.
const E_NOT_SUFFICIENT_BUFFER: HResult = 0x8007_007a_u32 as i32;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
    fn GetWindowsDirectoryW(buf: *mut u16, size: u32) -> u32;
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        sa: *const c_void,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> isize;
    fn GetFileSizeEx(h: isize, size: *mut i64) -> i32;
    fn ReadFile(h: isize, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> i32;
    fn CloseHandle(h: isize) -> i32;
    fn SwitchToThread() -> i32;
}

/// UTF-16 of an ASCII string, at compile time.
const fn wide<const N: usize>(s: &str) -> [u16; N] {
    let b = s.as_bytes();
    let mut out = [0u16; N];
    let mut i = 0;
    while i < b.len() && i < N {
        out[i] = b[i] as u16;
        i += 1;
    }
    out
}

const FONTS: usize = 4;
const FILE_NAMES: [&str; FONTS] = ["selawksl.ttf", "selawk.ttf", "selawksb.ttf", "selawkb.ttf"];
const WEIGHTS: [u32; FONTS] = [350, 400, 600, 700];
/// The regular face: what a family's best match falls back to.
const REGULAR: usize = 1;

// ---- Font data --------------------------------------------------------

/// Per font: 0 not read, 1 being read, 2 read, 3 unreadable.
static STATE: [AtomicU8; FONTS] = [const { AtomicU8::new(0) }; FONTS];
static DATA: [AtomicPtr<u8>; FONTS] = [const { AtomicPtr::new(core::ptr::null_mut()) }; FONTS];
static LEN: [AtomicUsize; FONTS] = [const { AtomicUsize::new(0) }; FONTS];

/// `%SystemRoot%\Fonts\<file>` of font `i`, NUL-terminated; its length.
fn path(i: usize, out: &mut [u16; 280]) -> usize {
    // SAFETY: our buffer.
    let mut n = unsafe { GetWindowsDirectoryW(out.as_mut_ptr(), 200) } as usize;
    if n == 0 || n >= 200 {
        n = 0;
        for &c in b"C:\\Windows" {
            out[n] = u16::from(c);
            n += 1;
        }
    }
    for &c in b"\\Fonts\\".iter().chain(FILE_NAMES[i].as_bytes()) {
        out[n] = u16::from(c);
        n += 1;
    }
    out[n] = 0;
    n
}

/// Reads font `i`'s file whole into the process heap.
fn read(i: usize) -> Option<(*mut u8, usize)> {
    let mut p = [0u16; 280];
    path(i, &mut p);
    const GENERIC_READ: u32 = 0x8000_0000;
    // SAFETY: a NUL-terminated path; plain calls on our handle and buffer.
    unsafe {
        let h = CreateFileW(p.as_ptr(), GENERIC_READ, 1, core::ptr::null(), 3, 0x80, 0);
        if h == -1 {
            return None;
        }
        let mut size = 0i64;
        let ok = GetFileSizeEx(h, &mut size) != 0 && (1..64 << 20).contains(&size);
        let buf =
            if ok { HeapAlloc(GetProcessHeap(), 0, size as usize) } else { core::ptr::null_mut() };
        let mut done = 0u32;
        let read = !buf.is_null()
            && ReadFile(h, buf, size as u32, &mut done, core::ptr::null_mut()) != 0
            && done as i64 == size;
        CloseHandle(h);
        if !read {
            if !buf.is_null() {
                HeapFree(GetProcessHeap(), 0, buf);
            }
            return None;
        }
        Some((buf, size as usize))
    }
}

/// Font `i`'s file contents (read on first use).
pub fn data(i: usize) -> Option<&'static [u8]> {
    loop {
        match STATE[i].load(Ordering::Acquire) {
            2 => {
                let (p, n) = (DATA[i].load(Ordering::Acquire), LEN[i].load(Ordering::Acquire));
                // SAFETY: a heap block of `n` bytes, never freed or written.
                return Some(unsafe { core::slice::from_raw_parts(p, n) });
            }
            3 => return None,
            0 if STATE[i].compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok() => {
                match read(i) {
                    Some((p, n)) => {
                        DATA[i].store(p, Ordering::Release);
                        LEN[i].store(n, Ordering::Release);
                        STATE[i].store(2, Ordering::Release);
                    }
                    None => {
                        missing(b"dwrite.dll!font file (Windows\\Fonts\\selawk*.ttf)", i as u64);
                        STATE[i].store(3, Ordering::Release);
                    }
                }
            }
            _ => {
                // SAFETY: plain call.
                unsafe { SwitchToThread() };
            }
        }
    }
}

fn face(i: usize) -> Option<ttf_parser::Face<'static>> {
    ttf_parser::Face::parse(data(i)?, 0).ok()
}

// ---- Static objects ---------------------------------------------------

#[repr(C)]
pub struct Obj {
    vtbl: *const (),
}

/// An object of font `index`.
#[repr(C)]
pub struct Indexed {
    vtbl: *const (),
    index: usize,
}

/// One localized string (en-us).
#[repr(C)]
pub struct Strings {
    vtbl: *const (),
    text: &'static [u16],
}

// SAFETY (the three): immutable pointers to static vtables and data.
unsafe impl Sync for Obj {}
unsafe impl Sync for Indexed {}
unsafe impl Sync for Strings {}

fn index_of(this: *const c_void) -> usize {
    // SAFETY: `this` is one of the static Indexed objects.
    unsafe { (*this.cast::<Indexed>()).index }
}

/// Writes a static object to an output pointer.
fn give(out: *mut *const c_void, object: *const c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's output.
    unsafe { *out = object };
    S_OK
}

static COLLECTION: Obj = Obj { vtbl: vt(&COLLECTION_VTBL) };
static FAMILY: Obj = Obj { vtbl: vt(&FAMILY_VTBL) };
static LOADER: Obj = Obj { vtbl: vt(&LOADER_VTBL) };

macro_rules! per_font {
    ($vtbl:ident) => {
        [
            Indexed { vtbl: vt(&$vtbl), index: 0 },
            Indexed { vtbl: vt(&$vtbl), index: 1 },
            Indexed { vtbl: vt(&$vtbl), index: 2 },
            Indexed { vtbl: vt(&$vtbl), index: 3 },
        ]
    };
}

static FONT_OBJS: [Indexed; FONTS] = per_font!(FONT_VTBL);
static FACE_OBJS: [Indexed; FONTS] = per_font!(FACE_VTBL);
static FILE_OBJS: [Indexed; FONTS] = per_font!(FILE_VTBL);
static STREAM_OBJS: [Indexed; FONTS] = per_font!(STREAM_VTBL);
/// The files' reference keys: the font index (a UINT32).
static KEYS: [u32; FONTS] = [0, 1, 2, 3];

static FAMILY_NAME: [u16; 7] = wide("Selawik");
static FACE_NAME_TEXT: [[u16; 9]; FONTS] =
    [wide("SemiLight"), wide("Regular"), wide("SemiBold"), wide("Bold")];
const FACE_NAME_LEN: [usize; FONTS] = [9, 7, 8, 4];
static FULL_NAME_TEXT: [[u16; 17]; FONTS] =
    [wide("Selawik Semilight"), wide("Selawik"), wide("Selawik Semibold"), wide("Selawik Bold")];
const FULL_NAME_LEN: [usize; FONTS] = [17, 7, 16, 12];
static PS_NAME_TEXT: [[u16; 17]; FONTS] =
    [wide("Selawik-Semilight"), wide("Selawik"), wide("Selawik-Semibold"), wide("Selawik-Bold")];
const PS_NAME_LEN: [usize; FONTS] = [17, 7, 16, 12];

static FAMILY_NAMES: Strings = Strings { vtbl: vt(&STRINGS_VTBL), text: &FAMILY_NAME };

macro_rules! strings {
    ($text:ident, $len:ident) => {
        [
            Strings { vtbl: vt(&STRINGS_VTBL), text: $text[0].split_at($len[0]).0 },
            Strings { vtbl: vt(&STRINGS_VTBL), text: $text[1].split_at($len[1]).0 },
            Strings { vtbl: vt(&STRINGS_VTBL), text: $text[2].split_at($len[2]).0 },
            Strings { vtbl: vt(&STRINGS_VTBL), text: $text[3].split_at($len[3]).0 },
        ]
    };
}

static FACE_NAMES: [Strings; FONTS] = strings!(FACE_NAME_TEXT, FACE_NAME_LEN);
static FULL_NAMES: [Strings; FONTS] = strings!(FULL_NAME_TEXT, FULL_NAME_LEN);
static PS_NAMES: [Strings; FONTS] = strings!(PS_NAME_TEXT, PS_NAME_LEN);

/// The system collection, if gala's fonts can be read.
pub fn collection() -> Option<*const c_void> {
    data(REGULAR)?;
    Some((&raw const COLLECTION).cast())
}

/// The font file object for `path` if it names one of gala's font files
/// (by file name).
pub fn file_by_path(path: *const u16) -> Option<*const c_void> {
    let mut n = 0;
    let mut start = 0;
    // SAFETY: the caller's NUL-terminated path.
    unsafe {
        while *path.add(n) != 0 {
            if *path.add(n) == u16::from(b'\\') || *path.add(n) == u16::from(b'/') {
                start = n + 1;
            }
            n += 1;
        }
    }
    // SAFETY: as above, characters start..n.
    let name = unsafe { core::slice::from_raw_parts(path.add(start), n - start) };
    let i = FILE_NAMES.iter().position(|f| {
        f.len() == name.len()
            && f.bytes().zip(name).all(|(a, &b)| u16::from(a.to_ascii_lowercase()) == lower(b))
    })?;
    data(i)?;
    Some((&raw const FILE_OBJS[i]).cast())
}

fn lower(c: u16) -> u16 {
    if (u16::from(b'A')..=u16::from(b'Z')).contains(&c) { c + 32 } else { c }
}

/// The face of a font file object of ours.
pub fn face_for_file(file: *const c_void) -> Option<*const c_void> {
    let i = FILE_OBJS.iter().position(|f| core::ptr::eq((f as *const Indexed).cast(), file))?;
    Some((&raw const FACE_OBJS[i]).cast())
}

// ---- IDWriteFontCollection ------------------------------------------------

static COLLECTION_VTBL: VTable<7> = VTable([
    collection_qi as *const (),
    crate::static_add_ref as *const (),
    crate::static_release as *const (),
    collection_family_count as *const (),
    collection_get_family as *const (),
    collection_find_family as *const (),
    collection_font_from_face as *const (),
]);

extern "system" fn collection_qi(
    this: *const c_void,
    riid: *const Iid,
    out: *mut *const c_void,
) -> HResult {
    query(this, &[IID_COLLECTION], riid, out)
}

extern "system" fn collection_family_count(_this: *const c_void) -> u32 {
    1
}

extern "system" fn collection_get_family(
    _this: *const c_void,
    index: u32,
    out: *mut *const c_void,
) -> HResult {
    if index != 0 {
        return E_INVALIDARG;
    }
    give(out, (&raw const FAMILY).cast())
}

/// The family names found as Selawik: its own, and the Windows faces it
/// stands in for (gdi32 does the same).
const FAMILY_NAMES_FOUND: [&str; 12] = [
    "Selawik",
    "Segoe UI",
    "Arial",
    "Helvetica",
    "Tahoma",
    "Verdana",
    "Microsoft Sans Serif",
    "MS Shell Dlg",
    "MS Shell Dlg 2",
    "Times New Roman",
    "Calibri",
    "sans-serif",
];

extern "system" fn collection_find_family(
    _this: *const c_void,
    name: *const u16,
    index: *mut u32,
    exists: *mut i32,
) -> HResult {
    if name.is_null() || index.is_null() || exists.is_null() {
        return E_INVALIDARG;
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated name.
    while unsafe { *name.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: as above.
    let given = unsafe { core::slice::from_raw_parts(name, n) };
    let found = FAMILY_NAMES_FOUND.iter().any(|f| {
        f.len() == given.len()
            && f.bytes().zip(given).all(|(a, &b)| u16::from(a.to_ascii_lowercase()) == lower(b))
    });
    // SAFETY: the caller's outputs.
    unsafe {
        *index = if found { 0 } else { u32::MAX };
        *exists = i32::from(found);
    }
    S_OK
}

extern "system" fn collection_font_from_face(
    _this: *const c_void,
    face: *const c_void,
    out: *mut *const c_void,
) -> HResult {
    match FACE_OBJS.iter().position(|f| core::ptr::eq((f as *const Indexed).cast(), face)) {
        Some(i) => give(out, (&raw const FONT_OBJS[i]).cast()),
        None => DWRITE_E_NOFONT,
    }
}

// ---- IDWriteFontFamily (and IDWriteFontList) --------------------------

static FAMILY_VTBL: VTable<9> = VTable([
    family_qi as *const (),
    crate::static_add_ref as *const (),
    crate::static_release as *const (),
    family_collection as *const (),
    family_font_count as *const (),
    family_get_font as *const (),
    family_names as *const (),
    family_first_matching as *const (),
    family_matching as *const (),
]);

extern "system" fn family_qi(
    this: *const c_void,
    riid: *const Iid,
    out: *mut *const c_void,
) -> HResult {
    query(this, &[IID_FAMILY, IID_FONT_LIST], riid, out)
}

extern "system" fn family_collection(_this: *const c_void, out: *mut *const c_void) -> HResult {
    give(out, (&raw const COLLECTION).cast())
}

extern "system" fn family_font_count(_this: *const c_void) -> u32 {
    FONTS as u32
}

extern "system" fn family_get_font(
    _this: *const c_void,
    index: u32,
    out: *mut *const c_void,
) -> HResult {
    match FONT_OBJS.get(index as usize) {
        Some(f) => give(out, (f as *const Indexed).cast()),
        None => E_INVALIDARG,
    }
}

extern "system" fn family_names(_this: *const c_void, out: *mut *const c_void) -> HResult {
    give(out, (&raw const FAMILY_NAMES).cast())
}

/// The font nearest weight `weight` (all are upright and of normal width).
fn nearest(weight: u32) -> usize {
    (0..FONTS).min_by_key(|&i| WEIGHTS[i].abs_diff(weight)).unwrap_or(REGULAR)
}

extern "system" fn family_first_matching(
    _this: *const c_void,
    weight: u32,
    _stretch: u32,
    _style: u32,
    out: *mut *const c_void,
) -> HResult {
    give(out, (&raw const FONT_OBJS[nearest(weight)]).cast())
}

/// GetMatchingFonts: the family's fonts as a font list (in the family's
/// order, not sorted by closeness to the request).
extern "system" fn family_matching(
    _this: *const c_void,
    _weight: u32,
    _stretch: u32,
    _style: u32,
    out: *mut *const c_void,
) -> HResult {
    give(out, (&raw const FAMILY).cast())
}

// ---- IDWriteFont ----------------------------------------------------------

static FONT_VTBL: VTable<14> = VTable([
    font_qi as *const (),
    crate::static_add_ref as *const (),
    crate::static_release as *const (),
    font_family as *const (),
    font_weight as *const (),
    font_stretch as *const (),
    font_style as *const (),
    font_is_symbol as *const (),
    font_face_names as *const (),
    font_informational as *const (),
    font_simulations as *const (),
    font_metrics as *const (),
    font_has_character as *const (),
    font_create_face as *const (),
]);

extern "system" fn font_qi(
    this: *const c_void,
    riid: *const Iid,
    out: *mut *const c_void,
) -> HResult {
    query(this, &[IID_FONT], riid, out)
}

extern "system" fn font_family(_this: *const c_void, out: *mut *const c_void) -> HResult {
    give(out, (&raw const FAMILY).cast())
}

extern "system" fn font_weight(this: *const c_void) -> u32 {
    WEIGHTS[index_of(this)]
}

/// DWRITE_FONT_STRETCH_NORMAL.
extern "system" fn font_stretch(_this: *const c_void) -> u32 {
    5
}

/// DWRITE_FONT_STYLE_NORMAL.
extern "system" fn font_style(_this: *const c_void) -> u32 {
    0
}

extern "system" fn font_is_symbol(_this: *const c_void) -> i32 {
    0
}

extern "system" fn font_face_names(this: *const c_void, out: *mut *const c_void) -> HResult {
    give(out, (&raw const FACE_NAMES[index_of(this)]).cast())
}

/// GetInformationalStrings: the Win32 family and subfamily names, the full
/// name and the PostScript name; other strings do not exist.
extern "system" fn font_informational(
    this: *const c_void,
    id: u32,
    out: *mut *const c_void,
    exists: *mut i32,
) -> HResult {
    if out.is_null() || exists.is_null() {
        return E_INVALIDARG;
    }
    let i = index_of(this);
    let strings: Option<*const Strings> = match id {
        11 | 13 => Some(&raw const FAMILY_NAMES), // WIN32_FAMILY_NAMES, PREFERRED_FAMILY_NAMES
        12 | 14 => Some(&raw const FACE_NAMES[i]),
        16 => Some(&raw const FULL_NAMES[i]),
        17 => Some(&raw const PS_NAMES[i]),
        _ => None,
    };
    // SAFETY: the caller's outputs.
    unsafe {
        *exists = i32::from(strings.is_some());
        *out = strings.map_or(core::ptr::null(), |s| s.cast());
    }
    S_OK
}

extern "system" fn font_simulations(_this: *const c_void) -> u32 {
    0
}

extern "system" fn font_metrics(this: *const c_void, out: *mut [u8; 20]) {
    if !out.is_null() {
        // SAFETY: the caller's DWRITE_FONT_METRICS.
        unsafe { *out = metrics(index_of(this)) };
    }
}

extern "system" fn font_has_character(
    this: *const c_void,
    value: u32,
    exists: *mut i32,
) -> HResult {
    if exists.is_null() {
        return E_INVALIDARG;
    }
    let has = char::from_u32(value)
        .and_then(|c| face(index_of(this))?.glyph_index(c))
        .is_some_and(|g| g.0 != 0);
    // SAFETY: the caller's output.
    unsafe { *exists = i32::from(has) };
    S_OK
}

extern "system" fn font_create_face(this: *const c_void, out: *mut *const c_void) -> HResult {
    let i = index_of(this);
    if data(i).is_none() {
        return crate::DWRITE_E_FILENOTFOUND;
    }
    give(out, (&raw const FACE_OBJS[i]).cast())
}

/// DWRITE_FONT_METRICS of font `i` (design units): the Windows ascent and
/// descent (OS/2), the line gap (hhea), cap and x heights, underline
/// (post) and strikethrough (OS/2).
fn metrics(i: usize) -> [u8; 20] {
    let mut m = [0u8; 20];
    let Some(f) = face(i) else { return m };
    let (ascent, descent) = match f.tables().os2 {
        Some(os2) => (os2.windows_ascender(), os2.windows_descender().unsigned_abs() as i16),
        None => (f.ascender(), f.descender().abs()),
    };
    let under = f.underline_metrics();
    let strike = f.strikeout_metrics();
    let fields: [i16; 10] = [
        f.units_per_em() as i16,
        ascent,
        descent,
        f.line_gap(),
        f.capital_height().unwrap_or(ascent),
        f.x_height().unwrap_or(ascent / 2),
        under.map_or(-(ascent / 10), |u| u.position),
        under.map_or(ascent / 20, |u| u.thickness),
        strike.map_or(ascent / 3, |s| s.position),
        strike.map_or(ascent / 20, |s| s.thickness),
    ];
    for (k, v) in fields.iter().enumerate() {
        m[2 * k..2 * k + 2].copy_from_slice(&v.to_le_bytes());
    }
    m
}

// ---- IDWriteLocalizedStrings ---------------------------------------------

static STRINGS_VTBL: VTable<9> = VTable([
    strings_qi as *const (),
    crate::static_add_ref as *const (),
    crate::static_release as *const (),
    strings_count as *const (),
    strings_find_locale as *const (),
    strings_locale_length as *const (),
    strings_locale as *const (),
    strings_length as *const (),
    strings_get as *const (),
]);

static LOCALE: [u16; 5] = wide("en-us");

extern "system" fn strings_qi(
    this: *const c_void,
    riid: *const Iid,
    out: *mut *const c_void,
) -> HResult {
    query(this, &[IID_STRINGS], riid, out)
}

extern "system" fn strings_count(_this: *const c_void) -> u32 {
    1
}

extern "system" fn strings_find_locale(
    _this: *const c_void,
    name: *const u16,
    index: *mut u32,
    exists: *mut i32,
) -> HResult {
    if name.is_null() || index.is_null() || exists.is_null() {
        return E_INVALIDARG;
    }
    let mut same = true;
    for (k, &c) in LOCALE.iter().enumerate() {
        // SAFETY: the caller's NUL-terminated name (stops at a mismatch).
        let given = unsafe { *name.add(k) };
        if lower(given) != c {
            same = false;
            break;
        }
    }
    // SAFETY: as above, then the caller's outputs.
    unsafe {
        same = same && *name.add(LOCALE.len()) == 0;
        *index = if same { 0 } else { u32::MAX };
        *exists = i32::from(same);
    }
    S_OK
}

fn copy_string(text: &[u16], buf: *mut u16, size: u32) -> HResult {
    if buf.is_null() {
        return E_INVALIDARG;
    }
    if (size as usize) < text.len() + 1 {
        if size > 0 {
            // SAFETY: the caller's buffer of at least one character.
            unsafe { *buf = 0 };
        }
        return E_NOT_SUFFICIENT_BUFFER;
    }
    // SAFETY: the caller's buffer of `size` characters.
    unsafe {
        core::ptr::copy_nonoverlapping(text.as_ptr(), buf, text.len());
        *buf.add(text.len()) = 0;
    }
    S_OK
}

extern "system" fn strings_locale_length(
    _this: *const c_void,
    index: u32,
    len: *mut u32,
) -> HResult {
    if index != 0 || len.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's output.
    unsafe { *len = LOCALE.len() as u32 };
    S_OK
}

extern "system" fn strings_locale(
    _this: *const c_void,
    index: u32,
    buf: *mut u16,
    size: u32,
) -> HResult {
    if index != 0 {
        return E_INVALIDARG;
    }
    copy_string(&LOCALE, buf, size)
}

fn text_of(this: *const c_void) -> &'static [u16] {
    // SAFETY: `this` is one of the static Strings objects.
    unsafe { (*this.cast::<Strings>()).text }
}

extern "system" fn strings_length(this: *const c_void, index: u32, len: *mut u32) -> HResult {
    if index != 0 || len.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's output.
    unsafe { *len = text_of(this).len() as u32 };
    S_OK
}

extern "system" fn strings_get(
    this: *const c_void,
    index: u32,
    buf: *mut u16,
    size: u32,
) -> HResult {
    if index != 0 {
        return E_INVALIDARG;
    }
    copy_string(text_of(this), buf, size)
}

// ---- IDWriteFontFace ------------------------------------------------------

static FACE_VTBL: VTable<18> = VTable([
    face_qi as *const (),
    crate::static_add_ref as *const (),
    crate::static_release as *const (),
    face_type as *const (),
    face_files as *const (),
    face_index as *const (),
    font_simulations as *const (),
    font_is_symbol as *const (),
    font_metrics as *const (),
    face_glyph_count as *const (),
    face_design_glyph_metrics as *const (),
    face_glyph_indices as *const (),
    face_try_get_table as *const (),
    face_release_table as *const (),
    face_glyph_run_outline as *const (),
    face_recommended_rendering_mode as *const (),
    face_gdi_metrics as *const (),
    face_gdi_glyph_metrics as *const (),
]);

extern "system" fn face_qi(
    this: *const c_void,
    riid: *const Iid,
    out: *mut *const c_void,
) -> HResult {
    query(this, &[IID_FACE], riid, out)
}

/// DWRITE_FONT_FACE_TYPE_TRUETYPE.
extern "system" fn face_type(_this: *const c_void) -> u32 {
    1
}

extern "system" fn face_files(
    this: *const c_void,
    count: *mut u32,
    files: *mut *const c_void,
) -> HResult {
    if count.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's count and (when given) array of *count files.
    unsafe {
        if !files.is_null() {
            if *count < 1 {
                return E_INVALIDARG;
            }
            *files = (&raw const FILE_OBJS[index_of(this)]).cast();
        }
        *count = 1;
    }
    S_OK
}

extern "system" fn face_index(_this: *const c_void) -> u32 {
    0
}

extern "system" fn face_glyph_count(this: *const c_void) -> u16 {
    face(index_of(this)).map_or(0, |f| f.number_of_glyphs())
}

/// DWRITE_GLYPH_METRICS (design units) of `glyph`.
fn glyph_metrics(f: &ttf_parser::Face<'_>, ascent: i32, descent: i32, glyph: u16) -> [i32; 7] {
    let g = ttf_parser::GlyphId(glyph);
    let advance = i32::from(f.glyph_hor_advance(g).unwrap_or(0));
    let height = ascent + descent;
    match f.glyph_bounding_box(g) {
        Some(b) => {
            let lsb = i32::from(f.glyph_hor_side_bearing(g).unwrap_or(b.x_min));
            let rsb = advance - lsb - (i32::from(b.x_max) - i32::from(b.x_min));
            let tsb = ascent - i32::from(b.y_max);
            let bsb = height - tsb - (i32::from(b.y_max) - i32::from(b.y_min));
            [lsb, advance, rsb, tsb, height, bsb, ascent]
        }
        // An empty glyph (a space): all advance, no ink.
        None => [0, advance, advance, 0, height, height, ascent],
    }
}

extern "system" fn face_design_glyph_metrics(
    this: *const c_void,
    glyphs: *const u16,
    count: u32,
    out: *mut [i32; 7],
    _sideways: i32,
) -> HResult {
    if (glyphs.is_null() || out.is_null()) && count != 0 {
        return E_INVALIDARG;
    }
    let Some(f) = face(index_of(this)) else { return crate::E_FAIL };
    let m = metrics(index_of(this));
    let ascent = i32::from(i16::from_le_bytes([m[2], m[3]]));
    let descent = i32::from(i16::from_le_bytes([m[4], m[5]]));
    for k in 0..count as usize {
        // SAFETY: the caller's arrays of `count` entries.
        unsafe { *out.add(k) = glyph_metrics(&f, ascent, descent, *glyphs.add(k)) };
    }
    S_OK
}

extern "system" fn face_glyph_indices(
    this: *const c_void,
    codepoints: *const u32,
    count: u32,
    out: *mut u16,
) -> HResult {
    if (codepoints.is_null() || out.is_null()) && count != 0 {
        return E_INVALIDARG;
    }
    let f = face(index_of(this));
    for k in 0..count as usize {
        // SAFETY: the caller's arrays of `count` entries.
        unsafe {
            let c = char::from_u32(*codepoints.add(k));
            *out.add(k) = c.and_then(|c| f.as_ref()?.glyph_index(c)).map_or(0, |g| g.0);
        }
    }
    S_OK
}

extern "system" fn face_try_get_table(
    this: *const c_void,
    tag: u32,
    table: *mut *const u8,
    size: *mut u32,
    context: *mut *mut c_void,
    exists: *mut i32,
) -> HResult {
    if table.is_null() || size.is_null() || context.is_null() || exists.is_null() {
        return E_INVALIDARG;
    }
    // DWRITE_MAKE_OPENTYPE_TAG puts the first character in the low byte.
    let found = data(index_of(this))
        .and_then(|d| ttf_parser::RawFace::parse(d, 0).ok())
        .and_then(|raw| raw.table(ttf_parser::Tag::from_bytes(&tag.to_le_bytes())));
    // SAFETY: the caller's outputs; the table lies in the static font data.
    unsafe {
        *context = core::ptr::null_mut();
        match found {
            Some(t) => {
                *table = t.as_ptr();
                *size = t.len() as u32;
                *exists = 1;
            }
            None => {
                *table = core::ptr::null();
                *size = 0;
                *exists = 0;
            }
        }
    }
    S_OK
}

extern "system" fn face_release_table(_this: *const c_void, _context: *mut c_void) {}

extern "system" fn face_glyph_run_outline(_this: *const c_void) -> HResult {
    missing(b"dwrite.dll!IDWriteFontFace::GetGlyphRunOutline", 0);
    E_NOTIMPL
}

/// DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC (anti-aliased, as gala draws).
extern "system" fn face_recommended_rendering_mode(
    _this: *const c_void,
    _em: f32,
    _pixels_per_dip: f32,
    _measuring: u32,
    _params: *const c_void,
    mode: *mut u32,
) -> HResult {
    if mode.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's output.
    unsafe { *mode = 5 };
    S_OK
}

/// GetGdiCompatibleMetrics: the design metrics (gala does not hint).
extern "system" fn face_gdi_metrics(
    this: *const c_void,
    _em: f32,
    _pixels_per_dip: f32,
    _transform: *const c_void,
    out: *mut [u8; 20],
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    font_metrics(this, out);
    S_OK
}

/// GetGdiCompatibleGlyphMetrics: the design metrics (gala does not hint).
#[allow(clippy::too_many_arguments)]
extern "system" fn face_gdi_glyph_metrics(
    this: *const c_void,
    _em: f32,
    _pixels_per_dip: f32,
    _transform: *const c_void,
    _natural: i32,
    glyphs: *const u16,
    count: u32,
    out: *mut [i32; 7],
    sideways: i32,
) -> HResult {
    face_design_glyph_metrics(this, glyphs, count, out, sideways)
}

// ---- IDWriteFontFile, the local loader, streams ---------------------------

static FILE_VTBL: VTable<6> = VTable([
    file_qi as *const (),
    crate::static_add_ref as *const (),
    crate::static_release as *const (),
    file_reference_key as *const (),
    file_loader as *const (),
    file_analyze as *const (),
]);

extern "system" fn file_qi(
    this: *const c_void,
    riid: *const Iid,
    out: *mut *const c_void,
) -> HResult {
    query(this, &[IID_FILE], riid, out)
}

extern "system" fn file_reference_key(
    this: *const c_void,
    key: *mut *const c_void,
    size: *mut u32,
) -> HResult {
    if key.is_null() || size.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's outputs.
    unsafe {
        *key = (&raw const KEYS[index_of(this)]).cast();
        *size = 4;
    }
    S_OK
}

extern "system" fn file_loader(_this: *const c_void, out: *mut *const c_void) -> HResult {
    give(out, (&raw const LOADER).cast())
}

/// Analyze: a supported TrueType file (DWRITE_FONT_FILE_TYPE_TRUETYPE, 2)
/// with one TrueType face.
extern "system" fn file_analyze(
    _this: *const c_void,
    supported: *mut i32,
    file_type: *mut u32,
    face_type: *mut u32,
    faces: *mut u32,
) -> HResult {
    if supported.is_null() || file_type.is_null() || faces.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's outputs (the face type is optional).
    unsafe {
        *supported = 1;
        *file_type = 2;
        if !face_type.is_null() {
            *face_type = 1;
        }
        *faces = 1;
    }
    S_OK
}

static LOADER_VTBL: VTable<7> = VTable([
    loader_qi as *const (),
    crate::static_add_ref as *const (),
    crate::static_release as *const (),
    loader_create_stream as *const (),
    loader_path_length as *const (),
    loader_path as *const (),
    loader_write_time as *const (),
]);

extern "system" fn loader_qi(
    this: *const c_void,
    riid: *const Iid,
    out: *mut *const c_void,
) -> HResult {
    query(this, &[IID_FILE_LOADER, IID_LOCAL_LOADER], riid, out)
}

/// The font a reference key names (our keys are a UINT32 index).
fn key_index(key: *const c_void, size: u32) -> Option<usize> {
    if key.is_null() || size != 4 {
        return None;
    }
    // SAFETY: a 4-byte key.
    let i = unsafe { key.cast::<u32>().read_unaligned() } as usize;
    (i < FONTS).then_some(i)
}

extern "system" fn loader_create_stream(
    _this: *const c_void,
    key: *const c_void,
    size: u32,
    out: *mut *const c_void,
) -> HResult {
    let Some(i) = key_index(key, size) else { return E_INVALIDARG };
    if data(i).is_none() {
        return crate::DWRITE_E_FILENOTFOUND;
    }
    give(out, (&raw const STREAM_OBJS[i]).cast())
}

extern "system" fn loader_path_length(
    _this: *const c_void,
    key: *const c_void,
    size: u32,
    len: *mut u32,
) -> HResult {
    let Some(i) = key_index(key, size) else { return E_INVALIDARG };
    if len.is_null() {
        return E_INVALIDARG;
    }
    let mut p = [0u16; 280];
    // SAFETY: the caller's output.
    unsafe { *len = path(i, &mut p) as u32 };
    S_OK
}

extern "system" fn loader_path(
    _this: *const c_void,
    key: *const c_void,
    size: u32,
    buf: *mut u16,
    len: u32,
) -> HResult {
    let Some(i) = key_index(key, size) else { return E_INVALIDARG };
    let mut p = [0u16; 280];
    let n = path(i, &mut p);
    copy_string(&p[..n], buf, len)
}

extern "system" fn loader_write_time(
    _this: *const c_void,
    key: *const c_void,
    size: u32,
    time: *mut u64,
) -> HResult {
    if key_index(key, size).is_none() || time.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's FILETIME (gala's font files never change).
    unsafe { time.write_unaligned(0) };
    S_OK
}

static STREAM_VTBL: VTable<7> = VTable([
    stream_qi as *const (),
    crate::static_add_ref as *const (),
    crate::static_release as *const (),
    stream_read as *const (),
    stream_release_fragment as *const (),
    stream_size as *const (),
    stream_write_time as *const (),
]);

extern "system" fn stream_qi(
    this: *const c_void,
    riid: *const Iid,
    out: *mut *const c_void,
) -> HResult {
    query(this, &[IID_STREAM], riid, out)
}

/// ReadFileFragment: a pointer into the file's data (it stays valid: the
/// data is never freed).
extern "system" fn stream_read(
    this: *const c_void,
    start: *mut *const u8,
    offset: u64,
    size: u64,
    context: *mut *mut c_void,
) -> HResult {
    if start.is_null() || context.is_null() {
        return E_INVALIDARG;
    }
    let Some(d) = data(index_of(this)) else { return crate::E_FAIL };
    let fits = offset.checked_add(size).is_some_and(|end| end <= d.len() as u64);
    // SAFETY: the caller's outputs; the fragment lies in the data.
    unsafe {
        *context = core::ptr::null_mut();
        if !fits {
            *start = core::ptr::null();
            return crate::E_FAIL;
        }
        *start = d.as_ptr().add(offset as usize);
    }
    S_OK
}

extern "system" fn stream_release_fragment(_this: *const c_void, _context: *mut c_void) {}

extern "system" fn stream_size(this: *const c_void, size: *mut u64) -> HResult {
    if size.is_null() {
        return E_INVALIDARG;
    }
    let Some(d) = data(index_of(this)) else { return crate::E_FAIL };
    // SAFETY: the caller's output.
    unsafe { *size = d.len() as u64 };
    S_OK
}

extern "system" fn stream_write_time(_this: *const c_void, time: *mut u64) -> HResult {
    if time.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's output.
    unsafe { *time = 0 };
    S_OK
}

#[allow(dead_code)]
const _: () = assert!(E_NOTIMPL != S_OK);
