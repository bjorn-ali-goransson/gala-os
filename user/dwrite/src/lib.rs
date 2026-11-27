//! gala's DWrite.dll (DirectWrite, dwrite.h): the system font collection
//! and the font objects programs reach from it, as Microsoft documents
//! them, over gala's fonts. Chromium's browser process makes a factory at
//! start-up and hands it to Skia's font manager (`CHECK(!!factory)`), and
//! its font lookup table enumerates the collection.
//!
//! The collection has one family, Selawik (an open font with Segoe UI's
//! metrics, the one gdi32 draws with), in four faces: SemiLight (350),
//! Regular (400), SemiBold (600) and Bold (700), read from
//! `C:\Windows\Fonts\selawk*.ttf`. As gdi32 does, the faces programs ask
//! for by name that Windows ships (Segoe UI, Arial, Times New Roman, ...)
//! are found as Selawik (`FindFamilyName`).
//!
//! Every object reachable from the system collection is static (one per
//! font, file, face and string set): their AddRef and Release count
//! nothing, as nothing is ever freed. Font data comes from the files,
//! read whole into the process heap on first use; TrueType tables are
//! read with `ttf-parser`.
//!
//! Custom font files and collections (Chromium's renderer proxies its
//! fonts through them: its loaders serve the browser's handles to the
//! system's font files) are read through the caller's loaders and found
//! by content among the system's fonts: CreateCustomFontFileReference
//! gives that font's file, CreateCustomFontCollection the system
//! collection. Other fonts are not supported (ledger).
//!
//! Text-rendering parameters (`params`): the factory's own set, a
//! monitor's and a custom one, in one object answering
//! IDWriteRenderingParams and the three interfaces that extend it. They
//! are what Direct2D's drawing state blocks carry, and Word reads them
//! before its first frame.
//!
//! Text analysis (`analyze`): the one text analyzer, which itemizes a
//! string into script, bidi and line-break runs through the caller's
//! analysis source and sink, maps characters to a face's glyphs and
//! scales each glyph's design advance by the em size. Word's start-up
//! stops at the first refused `CreateTextAnalyzer` with "something went
//! wrong" (error 47) and nothing else, so an analyzer refused is a
//! program that cannot start: what is not measured yet is answered as
//! well as it can be and says so to the ledger, rather than refused.
//!
//! Glyphs as pixels and as outlines (`glyphs`), which is what Skia's
//! DirectWrite scaler draws text from: CreateGlyphRunAnalysis rasterizes
//! a glyph run (fontdue, as gdi32 draws Selawik) into an aliased or
//! ClearType coverage texture, and GetGlyphRunOutline gives a glyph's
//! TrueType outline to a geometry sink. Rotation, skew and sideways runs
//! are not supported (ledger, E_NOTIMPL).
//!
//! Text formats (`format`): what a piece of text is laid out with --
//! the family, weight, style, stretch, em size and locale, and the
//! paragraph's alignment, wrapping, direction, tab stop, line spacing
//! and trimming -- with typography (a font's features) and number
//! substitution beside them. Word's start-up makes 279 formats in the
//! tenth of a second before its window, and reads back what it set.
//!
//! GDI logical fonts (`logfont`): what one of gala's fonts or faces is
//! as a LOGFONT, which is how a program carries a font through the GDI
//! half of its own drawing.
//!
//! Not there yet (each reported to the ledger, E_NOTIMPL): text layouts
//! (the formats above are what a layout is made from), ellipsis
//! trimming signs, the rest of the GDI interop object's methods (the
//! object itself answers for itself), and the IDWriteFactory1+
//! interfaces (QueryInterface says E_NOINTERFACE, which Skia accepts).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod analyze;
mod font;
mod format;
mod glyphs;
mod logfont;
mod params;

extern crate alloc;
use gala_heap as _;

use core::ffi::c_void;

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    // fontdue's hashing references it.
    fn strlen(s: *const u8) -> usize;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

pub type HResult = i32;
pub const S_OK: HResult = 0;
pub const E_NOTIMPL: HResult = 0x8000_4001_u32 as i32;
pub const E_NOINTERFACE: HResult = 0x8000_4002_u32 as i32;
pub const E_POINTER: HResult = 0x8000_4003_u32 as i32;
pub const E_INVALIDARG: HResult = 0x8007_0057_u32 as i32;
pub const E_OUTOFMEMORY: HResult = 0x8007_000e_u32 as i32;
pub const E_FAIL: HResult = 0x8000_4005_u32 as i32;
/// DWRITE_E_FILENOTFOUND.
pub const DWRITE_E_FILENOTFOUND: HResult = 0x8898_5003_u32 as i32;

/// Reports a gap (`what` with a detail) to the ledger.
pub fn missing(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
}

/// Records `dwrite.dll!<what>`: a call a program made and was answered,
/// the HRESULT as the detail (0 for success). A call that works leaves
/// nothing else behind, so a program that stops right after one shows
/// how far it got (OBSERVABILITY "Missing-features ledger").
pub fn answered(what: &[u8], hr: HResult) {
    let mut name = [0u8; 96];
    let mut n = 0;
    for &b in b"dwrite.dll!".iter().chain(what) {
        if n == name.len() {
            break;
        }
        name[n] = b;
        n += 1;
    }
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(name.as_ptr(), n, 5, hr as u32 as u64) };
}

/// A COM interface identifier, as the bytes of a GUID in memory.
pub type Iid = [u8; 16];

/// The bytes of a GUID given as DEFINE_GUID does.
pub const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Iid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

pub const IID_IUNKNOWN: Iid = guid(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
pub const IID_IDWRITE_FACTORY: Iid =
    guid(0xb859ee5a, 0xd838, 0x4b5b, [0xa2, 0xe8, 0x1a, 0xdc, 0x7d, 0x93, 0xdb, 0x48]);

/// The factory interfaces this DLL answers. DirectWrite answers
/// IDWriteFactory and every revision after it (IDWriteFactory1 through
/// 7, each only adding methods to the one before) with the one factory
/// object, from QueryInterface and from DWriteCreateFactory alike --
/// measured on Windows. gala answers a revision when the methods that
/// revision adds exist, and refuses it until then, because a revision
/// answered but not implemented is worse for a program than one
/// refused: a program asks for the revision it would like, and takes a
/// fallback when it is refused, while a method that answers E_NOTIMPL is
/// an answer it did not plan for. Measured too -- Chromium asks for
/// IDWriteFactory2, and with it answered its first call
/// (GetSystemFontFallback) ends the browser on a CHECK, where the
/// refusal costs it only its font fallback. The refusal names the whole
/// IID, so a run says which revision was wanted (dwrite_test step 11).
const FACTORY_IIDS: [Iid; 1] = [IID_IDWRITE_FACTORY];

/// The GDI interop interfaces, which the one interop object answers.
const GDI_INTEROP_IIDS: [Iid; 2] = [
    guid(0x1edd9491, 0x9853, 0x4299, [0x89, 0x8f, 0x64, 0x32, 0x98, 0x3b, 0x6f, 0x3a]),
    guid(0x4556be70, 0x3abd, 0x4f70, [0x90, 0xbe, 0x42, 0x17, 0x80, 0xa6, 0xf5, 0x15]),
];

/// A vtable: the methods' addresses, in the interface's order.
pub struct VTable<const N: usize>(pub [*const (); N]);
// SAFETY: function addresses only, never written.
unsafe impl<const N: usize> Sync for VTable<N> {}

/// A vtable's address, as objects hold it.
pub const fn vt<const N: usize>(v: &'static VTable<N>) -> *const () {
    (v as *const VTable<N>).cast()
}

/// The factory (one for the process: shared and isolated factories behave
/// the same, as gala's collection never changes).
#[repr(C)]
struct Factory {
    vtbl: *const (),
}
// SAFETY: an immutable pointer to a static vtable.
unsafe impl Sync for Factory {}

static FACTORY: Factory = Factory { vtbl: vt(&FACTORY_VTBL) };

static FACTORY_VTBL: VTable<57> = VTable([
    factory_qi as *const (),
    static_add_ref as *const (),
    static_release as *const (),
    factory_get_system_font_collection as *const (),
    factory_create_custom_font_collection as *const (),
    factory_register_loader as *const (),
    factory_register_loader as *const (),
    factory_create_font_file_reference as *const (),
    factory_create_custom_font_file_reference as *const (),
    factory_create_font_face as *const (),
    params::factory_CreateRenderingParams as *const (),
    params::factory_CreateMonitorRenderingParams as *const (),
    params::factory_CreateCustomRenderingParams as *const (),
    factory_register_loader as *const (),
    factory_register_loader as *const (),
    format::factory_CreateTextFormat as *const (),
    format::factory_CreateTypography as *const (),
    factory_GetGdiInterop as *const (),
    factory_CreateTextLayout as *const (),
    factory_CreateGdiCompatibleTextLayout as *const (),
    factory_CreateEllipsisTrimmingSign as *const (),
    analyze::factory_CreateTextAnalyzer as *const (),
    format::factory_CreateNumberSubstitution as *const (),
    glyphs::create_glyph_run_analysis as *const (),
    // IDWriteFactory1 and the revisions after it, in the order the
    // headers declare them, up to IDWriteFactory7's whole length: a
    // program that takes the factory for a later revision on the
    // Windows version alone, rather than asking for it, calls these
    // through this same vtable, and a slot that exists records the
    // method's name where a word past the end of the table would be
    // called as a function. They are also where a revision's methods
    // land when one is implemented and FACTORY_IIDS answers it.
    factory1_GetEudcFontCollection as *const (),
    factory1_CreateCustomRenderingParams as *const (),
    factory2_GetSystemFontFallback as *const (),
    factory2_CreateFontFallbackBuilder as *const (),
    factory2_TranslateColorGlyphRun as *const (),
    factory2_CreateCustomRenderingParams as *const (),
    factory2_CreateGlyphRunAnalysis as *const (),
    factory3_CreateGlyphRunAnalysis as *const (),
    factory3_CreateCustomRenderingParams as *const (),
    factory3_CreateFontFaceReference_file as *const (),
    factory3_CreateFontFaceReference_path as *const (),
    factory3_GetSystemFontSet as *const (),
    factory3_CreateFontSetBuilder as *const (),
    factory3_CreateFontCollectionFromFontSet as *const (),
    factory3_GetSystemFontCollection as *const (),
    factory3_GetFontDownloadQueue as *const (),
    factory4_TranslateColorGlyphRun as *const (),
    factory4_ComputeGlyphOrigins_baseline as *const (),
    factory4_ComputeGlyphOrigins_measuring as *const (),
    factory5_CreateFontSetBuilder as *const (),
    factory5_CreateInMemoryFontFileLoader as *const (),
    factory5_CreateHttpFontFileLoader as *const (),
    factory5_AnalyzeContainerType as *const (),
    factory5_UnpackFontFile as *const (),
    factory6_CreateFontFaceReference as *const (),
    factory6_CreateFontResource as *const (),
    factory6_GetSystemFontSet as *const (),
    factory6_GetSystemFontCollection as *const (),
    factory6_CreateFontCollectionFromFontSet as *const (),
    factory6_CreateFontSetBuilder as *const (),
    factory6_CreateTextFormat as *const (),
    factory7_GetSystemFontSet as *const (),
    factory7_GetSystemFontCollection as *const (),
]);

/// The GDI interop object (one for the factory, as Windows has it): the
/// bridge between a font and a LOGFONT, which Word asks the factory for
/// while it lays out its window's text. Its methods are not implemented;
/// each records its own name, so a run names the one a program wanted
/// rather than stopping with nothing said.
#[repr(C)]
struct GdiInterop {
    vtbl: *const (),
}
// SAFETY: an immutable pointer to a static vtable.
unsafe impl Sync for GdiInterop {}

static GDI_INTEROP: GdiInterop = GdiInterop { vtbl: vt(&GDI_INTEROP_VTBL) };

static GDI_INTEROP_VTBL: VTable<12> = VTable([
    interop_qi as *const (),
    static_add_ref as *const (),
    static_release as *const (),
    interop_CreateFontFromLOGFONT as *const (),
    logfont::interop_ConvertFontToLOGFONT as *const (),
    logfont::interop_ConvertFontFaceToLOGFONT as *const (),
    interop_CreateFontFaceFromHdc as *const (),
    interop_CreateBitmapRenderTarget as *const (),
    interop1_CreateFontFromLOGFONT as *const (),
    interop1_GetFontSignature_face as *const (),
    interop1_GetFontSignature_font as *const (),
    interop1_GetMatchingFontsByLOGFONT as *const (),
]);

extern "system" fn interop_qi(
    this: *const c_void,
    riid: *const Iid,
    out: *mut *const c_void,
) -> HResult {
    query(this, &GDI_INTEROP_IIDS, riid, out)
}

/// Records `dwrite.dll!<what>` the first time a program reaches the call
/// site: a call that works leaves nothing else behind, so the ledger
/// would go quiet exactly where a program stops in the middle of a path
/// this DLL answers. Once per site -- what it leaves is the path a
/// program took through this DLL, not a count, and a drawing loop pays
/// one relaxed atomic a call (OBSERVABILITY "Missing-features ledger").
#[macro_export]
macro_rules! reached {
    ($what:expr) => {{
        static SEEN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
        // A relaxed load once it has been recorded, not a locked
        // exchange: the call is on a drawing loop.
        if !SEEN.load(core::sync::atomic::Ordering::Relaxed)
            && !SEEN.swap(true, core::sync::atomic::Ordering::Relaxed)
        {
            $crate::reached_now($what);
        }
    }};
}

/// `reached!`'s record (ledger kind 9, "reached"): the first call that
/// reached a method, which is a path and not a gap -- nor a call
/// answered S_OK, which `answered` records. Call it through the macro,
/// which keeps it to the first time.
pub fn reached_now(what: &[u8]) {
    let mut name = [0u8; 96];
    let mut n = 0;
    for &b in b"dwrite.dll!".iter().chain(what) {
        if n == name.len() {
            break;
        }
        name[n] = b;
        n += 1;
    }
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(name.as_ptr(), n, 9, 0) };
}

/// Writes `object` to `out` if `riid` is one of `iids`.
pub fn query(
    object: *const c_void,
    iids: &[Iid],
    riid: *const Iid,
    out: *mut *const c_void,
) -> HResult {
    if out.is_null() || riid.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID and output.
    unsafe {
        if *riid == IID_IUNKNOWN || iids.contains(&*riid) {
            *out = object;
            return S_OK;
        }
        *out = core::ptr::null();
        let (name, n) = about(b"dwrite.dll!QueryInterface", &*riid, b"refused");
        missing(&name[..n], 0);
    }
    E_NOINTERFACE
}

/// The bytes of a GUID in string order (Data1..Data3 little-endian).
const GUID_ORDER: [usize; 16] = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];

/// `<call>({XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}): <why>`, the record a
/// call about an interface leaves. The whole IID and not its first
/// dword: the dword names an interface only to a reader who already has
/// a list to match it against, and the one who needs the record is the
/// one who has not.
pub fn about(call: &[u8], g: &Iid, why: &[u8]) -> ([u8; 128], usize) {
    let mut text = [0u8; 128];
    let mut n = 0;
    let mut put = |b: u8| {
        if n < text.len() {
            text[n] = b;
            n += 1;
        }
    };
    for &b in call {
        put(b);
    }
    put(b'(');
    put(b'{');
    for (i, &b) in GUID_ORDER.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            put(b'-');
        }
        let hex = |v: u8| b"0123456789ABCDEF"[usize::from(v & 15)];
        put(hex(g[b] >> 4));
        put(hex(g[b]));
    }
    put(b'}');
    put(b')');
    put(b':');
    put(b' ');
    for &b in why {
        put(b);
    }
    (text, n)
}

/// AddRef/Release of the static objects: they live for the process.
pub extern "system" fn static_add_ref(_this: *const c_void) -> u32 {
    2
}

pub extern "system" fn static_release(_this: *const c_void) -> u32 {
    1
}

extern "system" fn factory_qi(
    this: *const c_void,
    riid: *const Iid,
    out: *mut *const c_void,
) -> HResult {
    query(this, &FACTORY_IIDS, riid, out)
}

/// IDWriteFactory::GetGdiInterop (17): the factory's GDI interop object.
/// Windows gives the same object on every call of a factory, so gala's
/// is the one static object (dwrite_test step 11).
#[allow(non_snake_case)]
extern "system" fn factory_GetGdiInterop(_this: *const c_void, out: *mut *const c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    reached!(b"IDWriteFactory::GetGdiInterop");
    // SAFETY: the caller's output.
    unsafe { *out = (&raw const GDI_INTEROP).cast() };
    S_OK
}

extern "system" fn factory_get_system_font_collection(
    _this: *const c_void,
    out: *mut *const c_void,
    _check_for_updates: i32,
) -> HResult {
    reached!(b"IDWriteFactory::GetSystemFontCollection");
    if out.is_null() {
        return E_POINTER;
    }
    match font::collection() {
        Some(c) => {
            // SAFETY: the caller's output.
            unsafe { *out = c };
            S_OK
        }
        None => {
            // SAFETY: as above.
            unsafe { *out = core::ptr::null() };
            E_FAIL
        }
    }
}

/// CreateCustomFontCollection(loader, key, size, &collection): the
/// loader's files; gala's fonts are static, so a collection of the
/// system's font files is the system collection (Chromium's renderer
/// builds one per family it asks its browser for, which hands it the
/// system's files). Files of other fonts are left out (ledger); with none
/// of gala's: DWRITE_E_FILENOTFOUND.
extern "system" fn factory_create_custom_font_collection(
    this: *const c_void,
    loader: *const c_void,
    key: *const c_void,
    key_size: u32,
    out: *mut *const c_void,
) -> HResult {
    reached!(b"IDWriteFactory::CreateCustomFontCollection");
    if out.is_null() || loader.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's output.
    unsafe { *out = core::ptr::null() };
    match font::custom_collection(this, loader, key, key_size) {
        None => {
            missing(b"dwrite.dll!CreateCustomFontCollection(loader failed)", 0);
            E_FAIL
        }
        Some((collection, ours, others)) => {
            if others > 0 {
                missing(
                    b"dwrite.dll!CreateCustomFontCollection(not a gala font)",
                    u64::from(others),
                );
            }
            if ours == 0 {
                return DWRITE_E_FILENOTFOUND;
            }
            // SAFETY: as above.
            unsafe { *out = collection };
            S_OK
        }
    }
}

/// Register/UnregisterFontCollectionLoader and FontFileLoader: accepted
/// (the loaders are only used by the custom collections and files that
/// are not supported yet, which say so themselves).
extern "system" fn factory_register_loader(_this: *const c_void, loader: *const c_void) -> HResult {
    reached!(b"IDWriteFactory::Register/UnregisterLoader");
    if loader.is_null() { E_INVALIDARG } else { S_OK }
}

extern "system" fn factory_create_font_file_reference(
    _this: *const c_void,
    path: *const u16,
    _write_time: *const u64,
    out: *mut *const c_void,
) -> HResult {
    reached!(b"IDWriteFactory::CreateFontFileReference");
    if path.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    match font::file_by_path(path) {
        Some(f) => {
            // SAFETY: the caller's output.
            unsafe { *out = f };
            S_OK
        }
        None => {
            missing(b"dwrite.dll!CreateFontFileReference(not a gala font)", 0);
            // SAFETY: as above.
            unsafe { *out = core::ptr::null() };
            DWRITE_E_FILENOTFOUND
        }
    }
}

/// CreateCustomFontFileReference(key, size, loader, &file): the file of
/// the system font whose contents the loader's stream serves (read now;
/// gala's font objects are static), else DWRITE_E_FILENOTFOUND (ledger).
extern "system" fn factory_create_custom_font_file_reference(
    _this: *const c_void,
    key: *const c_void,
    key_size: u32,
    loader: *const c_void,
    out: *mut *const c_void,
) -> HResult {
    reached!(b"IDWriteFactory::CreateCustomFontFileReference");
    if out.is_null() || loader.is_null() {
        return E_INVALIDARG;
    }
    match font::file_from_loader(key, key_size, loader) {
        Some(file) => {
            // SAFETY: the caller's output.
            unsafe { *out = file };
            S_OK
        }
        None => {
            missing(b"dwrite.dll!CreateCustomFontFileReference(not a gala font)", 0);
            // SAFETY: as above.
            unsafe { *out = core::ptr::null() };
            DWRITE_E_FILENOTFOUND
        }
    }
}

extern "system" fn factory_create_font_face(
    _this: *const c_void,
    face_type: u32,
    files: u32,
    file_list: *const *const c_void,
    index: u32,
    simulations: u32,
    out: *mut *const c_void,
) -> HResult {
    reached!(b"IDWriteFactory::CreateFontFace");
    if out.is_null() || file_list.is_null() || files == 0 {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's array of `files` files.
    let first = unsafe { *file_list };
    match font::face_for_file(first) {
        Some(face) if index == 0 && simulations == 0 && face_type <= 1 => {
            // SAFETY: the caller's output.
            unsafe { *out = face };
            S_OK
        }
        _ => {
            missing(b"dwrite.dll!CreateFontFace(file or options)", u64::from(simulations));
            // SAFETY: as above.
            unsafe { *out = core::ptr::null() };
            E_NOTIMPL
        }
    }
}

/// The methods not implemented yet (they all return an HRESULT; the
/// caller's arguments are left alone, outputs included); each reports
/// its own name, interface and all, which is the one the header gives
/// it. A slot that exists and says so is the point: a program that
/// asked for IDWriteFactory7 and called a method of it would otherwise
/// read a word past the end of the vtable and call it.
macro_rules! not_impl {
    ($($name:ident as $shown:literal)*) => {
        $(
            #[allow(non_snake_case)]
            extern "system" fn $name(_this: *const c_void) -> HResult {
                missing(concat!("dwrite.dll!", $shown).as_bytes(), 0);
                E_NOTIMPL
            }
        )*
    };
}

not_impl! {
    factory_CreateTextLayout as "IDWriteFactory::CreateTextLayout"
    factory_CreateGdiCompatibleTextLayout as "IDWriteFactory::CreateGdiCompatibleTextLayout"
    factory_CreateEllipsisTrimmingSign as "IDWriteFactory::CreateEllipsisTrimmingSign"
    factory1_GetEudcFontCollection as "IDWriteFactory1::GetEudcFontCollection"
    factory1_CreateCustomRenderingParams as "IDWriteFactory1::CreateCustomRenderingParams"
    factory2_GetSystemFontFallback as "IDWriteFactory2::GetSystemFontFallback"
    factory2_CreateFontFallbackBuilder as "IDWriteFactory2::CreateFontFallbackBuilder"
    factory2_TranslateColorGlyphRun as "IDWriteFactory2::TranslateColorGlyphRun"
    factory2_CreateCustomRenderingParams as "IDWriteFactory2::CreateCustomRenderingParams"
    factory2_CreateGlyphRunAnalysis as "IDWriteFactory2::CreateGlyphRunAnalysis"
    factory3_CreateGlyphRunAnalysis as "IDWriteFactory3::CreateGlyphRunAnalysis"
    factory3_CreateCustomRenderingParams as "IDWriteFactory3::CreateCustomRenderingParams"
    factory3_CreateFontFaceReference_file as "IDWriteFactory3::CreateFontFaceReference(file)"
    factory3_CreateFontFaceReference_path as "IDWriteFactory3::CreateFontFaceReference(path)"
    factory3_GetSystemFontSet as "IDWriteFactory3::GetSystemFontSet"
    factory3_CreateFontSetBuilder as "IDWriteFactory3::CreateFontSetBuilder"
    factory3_CreateFontCollectionFromFontSet as "IDWriteFactory3::CreateFontCollectionFromFontSet"
    factory3_GetSystemFontCollection as "IDWriteFactory3::GetSystemFontCollection"
    factory3_GetFontDownloadQueue as "IDWriteFactory3::GetFontDownloadQueue"
    factory4_TranslateColorGlyphRun as "IDWriteFactory4::TranslateColorGlyphRun"
    factory4_ComputeGlyphOrigins_baseline as "IDWriteFactory4::ComputeGlyphOrigins(baseline)"
    factory4_ComputeGlyphOrigins_measuring as "IDWriteFactory4::ComputeGlyphOrigins(measuring)"
    factory5_CreateFontSetBuilder as "IDWriteFactory5::CreateFontSetBuilder"
    factory5_CreateInMemoryFontFileLoader as "IDWriteFactory5::CreateInMemoryFontFileLoader"
    factory5_CreateHttpFontFileLoader as "IDWriteFactory5::CreateHttpFontFileLoader"
    factory5_AnalyzeContainerType as "IDWriteFactory5::AnalyzeContainerType"
    factory5_UnpackFontFile as "IDWriteFactory5::UnpackFontFile"
    factory6_CreateFontFaceReference as "IDWriteFactory6::CreateFontFaceReference"
    factory6_CreateFontResource as "IDWriteFactory6::CreateFontResource"
    factory6_GetSystemFontSet as "IDWriteFactory6::GetSystemFontSet"
    factory6_GetSystemFontCollection as "IDWriteFactory6::GetSystemFontCollection"
    factory6_CreateFontCollectionFromFontSet as "IDWriteFactory6::CreateFontCollectionFromFontSet"
    factory6_CreateFontSetBuilder as "IDWriteFactory6::CreateFontSetBuilder"
    factory6_CreateTextFormat as "IDWriteFactory6::CreateTextFormat"
    factory7_GetSystemFontSet as "IDWriteFactory7::GetSystemFontSet"
    factory7_GetSystemFontCollection as "IDWriteFactory7::GetSystemFontCollection"
    interop_CreateFontFromLOGFONT as "IDWriteGdiInterop::CreateFontFromLOGFONT"
    interop_CreateFontFaceFromHdc as "IDWriteGdiInterop::CreateFontFaceFromHdc"
    interop_CreateBitmapRenderTarget as "IDWriteGdiInterop::CreateBitmapRenderTarget"
    interop1_CreateFontFromLOGFONT as "IDWriteGdiInterop1::CreateFontFromLOGFONT(collection)"
    interop1_GetFontSignature_face as "IDWriteGdiInterop1::GetFontSignature(face)"
    interop1_GetFontSignature_font as "IDWriteGdiInterop1::GetFontSignature(font)"
    interop1_GetMatchingFontsByLOGFONT as "IDWriteGdiInterop1::GetMatchingFontsByLOGFONT"
}

/// DWriteCreateFactory(factoryType, iid, **factory): the process's
/// factory, for IDWriteFactory, the revisions after it (IDWriteFactory1
/// through 7) or IUnknown -- the same object for every one of them, as
/// Windows gives.
#[unsafe(no_mangle)]
pub extern "system" fn DWriteCreateFactory(
    factory_type: u32,
    iid: *const Iid,
    factory: *mut *const c_void,
) -> HResult {
    if factory.is_null() || iid.is_null() {
        return E_POINTER;
    }
    if factory_type > 1 {
        // SAFETY: the caller's output.
        unsafe { *factory = core::ptr::null() };
        return E_INVALIDARG;
    }
    let r = query((&raw const FACTORY).cast(), &FACTORY_IIDS, iid, factory);
    if r != S_OK {
        // SAFETY: the caller's IID, which query() read as well.
        let (name, n) =
            unsafe { about(b"dwrite.dll!DWriteCreateFactory", &*iid, b"no such interface") };
        missing(&name[..n], 0);
    }
    r
}

/// The DLL entry point: nothing to set up.
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
