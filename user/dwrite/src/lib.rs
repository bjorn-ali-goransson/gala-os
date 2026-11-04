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
//! Not there yet (each reported to the ledger, E_NOTIMPL): custom font
//! collections and file loaders (Chromium's renderer proxies its fonts
//! through them: BR stage 1's next step), text formats and layouts,
//! glyph run analysis (rasterization) and outlines, rendering params,
//! GDI interop, and the IDWriteFactory1+ interfaces (QueryInterface says
//! E_NOINTERFACE, which Skia accepts).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod font;

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
pub const E_FAIL: HResult = 0x8000_4005_u32 as i32;
/// DWRITE_E_FILENOTFOUND.
pub const DWRITE_E_FILENOTFOUND: HResult = 0x8898_5003_u32 as i32;

/// Reports a gap (`what` with a detail) to the ledger.
pub fn missing(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
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

static FACTORY_VTBL: VTable<24> = VTable([
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
    factory_CreateRenderingParams as *const (),
    factory_CreateMonitorRenderingParams as *const (),
    factory_CreateCustomRenderingParams as *const (),
    factory_register_loader as *const (),
    factory_register_loader as *const (),
    factory_CreateTextFormat as *const (),
    factory_CreateTypography as *const (),
    factory_GetGdiInterop as *const (),
    factory_CreateTextLayout as *const (),
    factory_CreateGdiCompatibleTextLayout as *const (),
    factory_CreateEllipsisTrimmingSign as *const (),
    factory_CreateTextAnalyzer as *const (),
    factory_CreateNumberSubstitution as *const (),
    factory_CreateGlyphRunAnalysis as *const (),
]);

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
    }
    E_NOINTERFACE
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
    query(this, &[IID_IDWRITE_FACTORY], riid, out)
}

extern "system" fn factory_get_system_font_collection(
    _this: *const c_void,
    out: *mut *const c_void,
    _check_for_updates: i32,
) -> HResult {
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

extern "system" fn factory_create_custom_font_collection(
    _this: *const c_void,
    _loader: *const c_void,
    _key: *const c_void,
    _key_size: u32,
    out: *mut *const c_void,
) -> HResult {
    missing(b"dwrite.dll!IDWriteFactory::CreateCustomFontCollection", 0);
    if !out.is_null() {
        // SAFETY: the caller's output.
        unsafe { *out = core::ptr::null() };
    }
    E_NOTIMPL
}

/// Register/UnregisterFontCollectionLoader and FontFileLoader: accepted
/// (the loaders are only used by the custom collections and files that
/// are not supported yet, which say so themselves).
extern "system" fn factory_register_loader(_this: *const c_void, loader: *const c_void) -> HResult {
    if loader.is_null() { E_INVALIDARG } else { S_OK }
}

extern "system" fn factory_create_font_file_reference(
    _this: *const c_void,
    path: *const u16,
    _write_time: *const u64,
    out: *mut *const c_void,
) -> HResult {
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

extern "system" fn factory_create_custom_font_file_reference(
    _this: *const c_void,
    _key: *const c_void,
    _key_size: u32,
    _loader: *const c_void,
    out: *mut *const c_void,
) -> HResult {
    missing(b"dwrite.dll!IDWriteFactory::CreateCustomFontFileReference", 0);
    if !out.is_null() {
        // SAFETY: the caller's output.
        unsafe { *out = core::ptr::null() };
    }
    E_NOTIMPL
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

/// The factory methods not implemented yet (they all return an HRESULT;
/// the caller's arguments are left alone, outputs included); each reports
/// its own name.
macro_rules! factory_not_impl {
    ($($name:ident)*) => {
        $(
            #[allow(non_snake_case)]
            extern "system" fn $name(_this: *const c_void) -> HResult {
                missing(concat!("dwrite.dll!IDWriteFactory::", stringify!($name)).as_bytes(), 0);
                E_NOTIMPL
            }
        )*
    };
}

factory_not_impl! {
    factory_CreateRenderingParams
    factory_CreateMonitorRenderingParams
    factory_CreateCustomRenderingParams
    factory_CreateTextFormat
    factory_CreateTypography
    factory_GetGdiInterop
    factory_CreateTextLayout
    factory_CreateGdiCompatibleTextLayout
    factory_CreateEllipsisTrimmingSign
    factory_CreateTextAnalyzer
    factory_CreateNumberSubstitution
    factory_CreateGlyphRunAnalysis
}

/// DWriteCreateFactory(factoryType, iid, **factory): the process's
/// factory, for IDWriteFactory (or IUnknown); E_NOINTERFACE for the later
/// factory interfaces (IDWriteFactory1..8), which gala lacks.
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
    let r = query((&raw const FACTORY).cast(), &[IID_IDWRITE_FACTORY], iid, factory);
    if r != S_OK {
        // SAFETY: the caller's IID: its first four bytes as the detail.
        let d1 = unsafe { u64::from(iid.cast::<u32>().read_unaligned()) };
        missing(b"dwrite.dll!DWriteCreateFactory(interface)", d1);
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
