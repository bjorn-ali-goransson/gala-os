//! OleCreateFontIndirect (#420): the standard font object (StdFont), as
//! documented for it and for IFont, with the details Wine's olefont.c
//! tests record of Windows: any cbSizeofstruct is taken; the size is a
//! CY in points (x 10000); Bold is a weight above 550 and sets 700 or
//! 400; the HFONT (get_hFont, made on first use and again after a
//! change; the object keeps it) is the face, weight, italic, underline,
//! strike-out and character set at a height of the size in points at
//! the ratio's pixels per inch: -(size x cyLogical x 635 / (cyHimetric x
//! 18)) / 10000, rounded up past half; the ratio is the screen's
//! LOGPIXELSY over 2540 until SetRatio. Interfaces: IUnknown and IFont;
//! IDispatch (IFontDisp), IPersistStream and the connection points are
//! E_NOINTERFACE (recorded). QueryTextMetrics is E_NOTIMPL (recorded).
//! Office's Click-to-Run UI imports it by ordinal.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use super::{
    E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, HResult, S_OK, SysAllocString, alloc_zeroed, free,
    missing, put,
};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
}

const E_NOINTERFACE: HResult = 0x8000_4002u32 as i32;
const E_POINTER: HResult = 0x8000_4003u32 as i32;
const E_FAIL: HResult = 0x8000_4005u32 as i32;
const S_FALSE: HResult = 1;
const CTL_E_INVALIDPROPERTYVALUE: HResult = 0x800a_017cu32 as i32;
const FACE: usize = 32;

type Guid = [u8; 16];
const IID_IUNKNOWN: Guid = [0, 0, 0, 0, 0, 0, 0, 0, 0xc0, 0, 0, 0, 0, 0, 0, 0x46];
/// {BEF6E002-A874-101A-8BBA-00AA00300CAB}
const IID_IFONT: Guid = [
    0x02, 0xe0, 0xf6, 0xbe, 0x74, 0xa8, 0x1a, 0x10, 0x8b, 0xba, 0x00, 0xaa, 0x00, 0x30, 0x0c, 0xab,
];

/// FONTDESC.
#[repr(C)]
pub struct FontDesc {
    size: u32,
    name: *const u16,
    cy: i64,
    weight: i16,
    charset: i16,
    italic: i32,
    underline: i32,
    strikethrough: i32,
}

#[repr(C)]
struct Font {
    vtable: *const *const (),
    refs: AtomicU32,
    name: [u16; FACE],
    cy: i64,
    weight: i16,
    charset: i16,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    logical: i32,
    himetric: i32,
    hfont: usize,
}

struct VTable([*const (); 27]);
// SAFETY: function addresses only.
unsafe impl Sync for VTable {}

static VTABLE: VTable = VTable([
    query as *const (),
    add_ref as *const (),
    release as *const (),
    get_name as *const (),
    put_name as *const (),
    get_size as *const (),
    put_size as *const (),
    get_bold as *const (),
    put_bold as *const (),
    get_italic as *const (),
    put_italic as *const (),
    get_underline as *const (),
    put_underline as *const (),
    get_strikethrough as *const (),
    put_strikethrough as *const (),
    get_weight as *const (),
    put_weight as *const (),
    get_charset as *const (),
    put_charset as *const (),
    get_hfont as *const (),
    clone as *const (),
    is_equal as *const (),
    set_ratio as *const (),
    query_text_metrics as *const (),
    add_ref_hfont as *const (),
    release_hfont as *const (),
    set_hdc as *const (),
]);

/// gdi32's or user32's function `name`, loaded on first use (programs
/// that never draw do not load them through oleaut32).
fn proc_of(dll: &str, name: &core::ffi::CStr) -> usize {
    let mut w = [0u16; 16];
    for (d, b) in w.iter_mut().zip(dll.bytes()) {
        *d = b.into();
    }
    // SAFETY: NUL-terminated names.
    unsafe {
        let m = LoadLibraryW(w.as_ptr());
        if m == 0 { 0 } else { GetProcAddress(m, name.as_ptr().cast()) }
    }
}

/// The screen's pixels per inch (LOGPIXELSY), 96 without a screen DC.
fn screen_dpi() -> i32 {
    let (get_dc, caps, rel) = (
        proc_of("user32.dll", c"GetDC"),
        proc_of("gdi32.dll", c"GetDeviceCaps"),
        proc_of("user32.dll", c"ReleaseDC"),
    );
    if get_dc == 0 || caps == 0 || rel == 0 {
        return 96;
    }
    // SAFETY: the documented signatures.
    unsafe {
        let get_dc: extern "system" fn(usize) -> usize = core::mem::transmute(get_dc);
        let caps: extern "system" fn(usize, i32) -> i32 = core::mem::transmute(caps);
        let rel: extern "system" fn(usize, usize) -> i32 = core::mem::transmute(rel);
        let dc = get_dc(0);
        let dpi = if dc == 0 { 0 } else { caps(dc, 90) };
        if dc != 0 {
            rel(0, dc);
        }
        if dpi > 0 { dpi } else { 96 }
    }
}

fn this<'a>(p: *mut Font) -> &'a mut Font {
    // SAFETY: a font object of ours.
    unsafe { &mut *p }
}

/// The HFONT goes (a property changed; the next get_hFont makes one).
fn forget_hfont(f: &mut Font) {
    if f.hfont != 0 {
        let delete = proc_of("gdi32.dll", c"DeleteObject");
        if delete != 0 {
            // SAFETY: DeleteObject's signature; our font.
            let delete: extern "system" fn(usize) -> i32 = unsafe { core::mem::transmute(delete) };
            delete(f.hfont);
        }
        f.hfont = 0;
    }
}

fn new_font(from: &Font) -> *mut Font {
    let p = alloc_zeroed(size_of::<Font>()).cast::<Font>();
    if !p.is_null() {
        // SAFETY: a new block of a Font's size.
        unsafe {
            p.write(Font {
                vtable: VTABLE.0.as_ptr(),
                refs: AtomicU32::new(1),
                name: from.name,
                cy: from.cy,
                weight: from.weight,
                charset: from.charset,
                italic: from.italic,
                underline: from.underline,
                strikethrough: from.strikethrough,
                logical: from.logical,
                himetric: from.himetric,
                hfont: 0,
            })
        };
    }
    p
}

/// OleCreateFontIndirect(lpFontDesc, riid, lplpvObj).
#[unsafe(no_mangle)]
pub extern "system" fn OleCreateFontIndirect(
    desc: *const FontDesc,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if iid.is_null() {
        return E_INVALIDARG;
    }
    let mut proto = Font {
        vtable: VTABLE.0.as_ptr(),
        refs: AtomicU32::new(1),
        name: [0; FACE],
        cy: 80_000, // 8 points: "System" when no description is given
        weight: 400,
        charset: 0,
        italic: false,
        underline: false,
        strikethrough: false,
        logical: screen_dpi(),
        himetric: 2540,
        hfont: 0,
    };
    for (d, c) in proto.name.iter_mut().zip("System".encode_utf16()) {
        *d = c;
    }
    if !desc.is_null() {
        // SAFETY: the caller's FONTDESC (its size is not checked, as on
        // Windows).
        let d = unsafe { &*desc };
        proto.name = [0; FACE];
        if !d.name.is_null() {
            for (i, slot) in proto.name.iter_mut().take(FACE - 1).enumerate() {
                // SAFETY: the caller's NUL-terminated face name.
                let c = unsafe { *d.name.add(i) };
                if c == 0 {
                    break;
                }
                *slot = c;
            }
        }
        proto.cy = d.cy;
        proto.weight = d.weight;
        proto.charset = d.charset;
        proto.italic = d.italic != 0;
        proto.underline = d.underline != 0;
        proto.strikethrough = d.strikethrough != 0;
    }
    let font = new_font(&proto);
    if font.is_null() {
        return E_OUTOFMEMORY;
    }
    let hr = query(font, iid, out);
    release(font);
    hr
}

extern "system" fn query(p: *mut Font, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's IID.
    let id = unsafe { *iid };
    if id == IID_IUNKNOWN || id == IID_IFONT {
        add_ref(p);
        put(out, p.cast());
        return S_OK;
    }
    put(out, core::ptr::null_mut());
    missing(
        b"oleaut32.dll!StdFont::QueryInterface",
        u64::from(u32::from_le_bytes([id[0], id[1], id[2], id[3]])),
    );
    E_NOINTERFACE
}

extern "system" fn add_ref(p: *mut Font) -> u32 {
    this(p).refs.fetch_add(1, Ordering::AcqRel) + 1
}

extern "system" fn release(p: *mut Font) -> u32 {
    let left = this(p).refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        forget_hfont(this(p));
        free(p.cast());
    }
    left
}

extern "system" fn get_name(p: *mut Font, out: *mut *mut u16) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let f = this(p);
    let mut name = [0u16; FACE + 1];
    name[..FACE].copy_from_slice(&f.name);
    let s = SysAllocString(name.as_ptr());
    put(out, s);
    if s.is_null() { E_OUTOFMEMORY } else { S_OK }
}

extern "system" fn put_name(p: *mut Font, name: *const u16) -> HResult {
    if name.is_null() {
        return CTL_E_INVALIDPROPERTYVALUE;
    }
    let f = this(p);
    f.name = [0; FACE];
    for (i, slot) in f.name.iter_mut().take(FACE - 1).enumerate() {
        // SAFETY: the caller's NUL-terminated BSTR.
        let c = unsafe { *name.add(i) };
        if c == 0 {
            break;
        }
        *slot = c;
    }
    forget_hfont(f);
    S_OK
}

extern "system" fn get_size(p: *mut Font, out: *mut i64) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, this(p).cy);
    S_OK
}

extern "system" fn put_size(p: *mut Font, cy: i64) -> HResult {
    let f = this(p);
    f.cy = cy;
    forget_hfont(f);
    S_OK
}

/// A BOOL property's getter (IFont's are BOOLs: TRUE is 1).
fn get_flag(out: *mut i32, v: bool) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, i32::from(v));
    S_OK
}

extern "system" fn get_bold(p: *mut Font, out: *mut i32) -> HResult {
    get_flag(out, this(p).weight > 550)
}

extern "system" fn put_bold(p: *mut Font, v: i32) -> HResult {
    let f = this(p);
    f.weight = if v != 0 { 700 } else { 400 };
    forget_hfont(f);
    S_OK
}

extern "system" fn get_italic(p: *mut Font, out: *mut i32) -> HResult {
    get_flag(out, this(p).italic)
}

extern "system" fn put_italic(p: *mut Font, v: i32) -> HResult {
    let f = this(p);
    f.italic = v != 0;
    forget_hfont(f);
    S_OK
}

extern "system" fn get_underline(p: *mut Font, out: *mut i32) -> HResult {
    get_flag(out, this(p).underline)
}

extern "system" fn put_underline(p: *mut Font, v: i32) -> HResult {
    let f = this(p);
    f.underline = v != 0;
    forget_hfont(f);
    S_OK
}

extern "system" fn get_strikethrough(p: *mut Font, out: *mut i32) -> HResult {
    get_flag(out, this(p).strikethrough)
}

extern "system" fn put_strikethrough(p: *mut Font, v: i32) -> HResult {
    let f = this(p);
    f.strikethrough = v != 0;
    forget_hfont(f);
    S_OK
}

extern "system" fn get_weight(p: *mut Font, out: *mut i16) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, this(p).weight);
    S_OK
}

extern "system" fn put_weight(p: *mut Font, v: i16) -> HResult {
    let f = this(p);
    f.weight = v;
    forget_hfont(f);
    S_OK
}

extern "system" fn get_charset(p: *mut Font, out: *mut i16) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, this(p).charset);
    S_OK
}

extern "system" fn put_charset(p: *mut Font, v: i16) -> HResult {
    let f = this(p);
    f.charset = v;
    forget_hfont(f);
    S_OK
}

/// The LOGFONTW height of the font's size at its ratio.
fn height(f: &Font) -> i32 {
    let num = i128::from(f.cy) * i128::from(f.logical) * 635;
    let den = i128::from(f.himetric.max(1)) * 18;
    let h = num / den;
    let (q, r) = (h / 10000, h % 10000);
    (if r > 5000 { -q - 1 } else { -q }) as i32
}

extern "system" fn get_hfont(p: *mut Font, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let f = this(p);
    if f.hfont == 0 {
        let create = proc_of("gdi32.dll", c"CreateFontIndirectW");
        if create == 0 {
            return E_FAIL;
        }
        // LOGFONTW: height, width, escapement, orientation, weight, italic,
        // underline, strike-out, charset, precisions, quality, pitch, face.
        let mut lf = [0u8; 92];
        lf[0..4].copy_from_slice(&height(f).to_le_bytes());
        lf[16..20].copy_from_slice(&i32::from(f.weight).to_le_bytes());
        lf[20] = u8::from(f.italic);
        lf[21] = u8::from(f.underline);
        lf[22] = u8::from(f.strikethrough);
        lf[23] = f.charset as u8;
        for (i, &c) in f.name.iter().enumerate() {
            lf[28 + 2 * i..30 + 2 * i].copy_from_slice(&c.to_le_bytes());
        }
        // SAFETY: CreateFontIndirectW's signature; our LOGFONTW.
        let create: extern "system" fn(*const u8) -> usize =
            unsafe { core::mem::transmute(create) };
        f.hfont = create(lf.as_ptr());
        if f.hfont == 0 {
            return E_FAIL;
        }
    }
    put(out, f.hfont);
    S_OK
}

extern "system" fn clone(p: *mut Font, out: *mut *mut Font) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let c = new_font(this(p));
    put(out, c);
    if c.is_null() { E_OUTOFMEMORY } else { S_OK }
}

/// IsEqual: S_OK when the other font has the same face (ignoring case is
/// not documented: compared as is), size and attributes, else S_FALSE.
extern "system" fn is_equal(p: *mut Font, other: *mut Font) -> HResult {
    if other.is_null() {
        return E_POINTER;
    }
    // Only our own objects can be compared field by field.
    // SAFETY: the other font's vtable pointer.
    if unsafe { (*other).vtable } != VTABLE.0.as_ptr() {
        return S_FALSE;
    }
    let (a, b) = (this(p), this(other));
    let same = a.name == b.name
        && a.cy == b.cy
        && a.weight == b.weight
        && a.charset == b.charset
        && a.italic == b.italic
        && a.underline == b.underline
        && a.strikethrough == b.strikethrough;
    if same { S_OK } else { S_FALSE }
}

extern "system" fn set_ratio(p: *mut Font, logical: i32, himetric: i32) -> HResult {
    if logical == 0 || himetric == 0 {
        return E_FAIL;
    }
    let f = this(p);
    f.logical = logical;
    f.himetric = himetric;
    forget_hfont(f);
    S_OK
}

extern "system" fn query_text_metrics(_p: *mut Font, _out: *mut c_void) -> HResult {
    missing(b"oleaut32.dll!StdFont::QueryTextMetrics", 0);
    E_NOTIMPL
}

/// AddRefHfont/ReleaseHfont: the font object keeps its HFONT whatever the
/// count; S_OK for its own, S_FALSE for another.
extern "system" fn add_ref_hfont(p: *mut Font, h: usize) -> HResult {
    if h != 0 && h == this(p).hfont { S_OK } else { S_FALSE }
}

extern "system" fn release_hfont(p: *mut Font, h: usize) -> HResult {
    add_ref_hfont(p, h)
}

extern "system" fn set_hdc(_p: *mut Font, _hdc: usize) -> HResult {
    S_OK
}
