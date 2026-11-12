//! gala's WindowsCodecs.dll: the Windows Imaging Component (WIC,
//! wincodec.h), the part programs decode and prepare images with
//! (ARCHITECTURE "windowscodecs"). Office imports
//! WICCreateImagingFactory_Proxy and WICConvertBitmapSource and decodes
//! its user-interface images through WIC (ROADMAP OF); Direct2D draws
//! WIC bitmaps.
//!
//! - The imaging factory (`factory`): one static object, made by
//!   WICCreateImagingFactory_Proxy or by COM (CLSID_WICImagingFactory
//!   and ...Factory2, registered in the registry seed as served by this
//!   DLL; DllGetClassObject). It answers IWICImagingFactory and
//!   IWICImagingFactory2.
//! - Streams (`stream`): IWICStream over the caller's memory, a file
//!   (read whole when it is opened) or another IStream.
//! - Decoders (`decoder`): PNG (gdiplus's `png.rs`, its samples kept in
//!   the PNG's own layout: the native pixel formats WIC documents) and
//!   BMP (`bmp`); one frame each, decoded whole at Initialize.
//! - Bitmaps and locks (`bitmap`), palettes (`palette`), and the sources
//!   that wrap another source (`transform`): the format converter, the
//!   scaler and the clipper. A source can be any object with the
//!   IWICBitmapSource methods, the caller's own included (`Source`).
//! - Pixel formats and their conversion (`format`).
//!
//! What it does not do fails as Windows would for a missing component
//! and is recorded in the ledger (`windowscodecs.dll!...`): other image
//! formats (JPEG, GIF, TIFF, ICO, ...), encoders, metadata, colour
//! contexts and transforms, component information, bitmaps from GDI
//! objects, conversion to indexed and grey formats.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;

mod bitmap;
mod bmp;
mod decoder;
mod factory;
mod format;
mod palette;
// WIC's PNG decoder: gdiplus's, one source for three DLLs (its ARGB
// output, `decode`, is gdiplus's and user32's; this DLL reads the
// samples, `decode_raw`).
#[allow(dead_code)]
#[path = "../../gdiplus/src/png.rs"]
mod png;
mod stream;
mod transform;

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use gala_heap as _;

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

// The memory routines come from gala's ntdll (as other DLLs on `alloc`
// take them).
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
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

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn ExitProcess(code: u32) -> !;
}

pub type HResult = i32;
pub type Guid = [u8; 16];
pub type Obj = *mut c_void;

pub const S_OK: HResult = 0;
pub const S_FALSE: HResult = 1;
pub const E_NOTIMPL: HResult = 0x8000_4001_u32 as i32;
pub const E_NOINTERFACE: HResult = 0x8000_4002_u32 as i32;
pub const E_POINTER: HResult = 0x8000_4003_u32 as i32;
pub const E_FAIL: HResult = 0x8000_4005_u32 as i32;
pub const E_OUTOFMEMORY: HResult = 0x8007_000e_u32 as i32;
pub const E_INVALIDARG: HResult = 0x8007_0057_u32 as i32;
pub const CLASS_E_NOAGGREGATION: HResult = 0x8004_0110_u32 as i32;
pub const CLASS_E_CLASSNOTAVAILABLE: HResult = 0x8004_0111_u32 as i32;
pub const WINCODEC_ERR_WRONGSTATE: HResult = 0x8898_2f04_u32 as i32;
pub const WINCODEC_ERR_UNKNOWNIMAGEFORMAT: HResult = 0x8898_2f07_u32 as i32;
pub const WINCODEC_ERR_NOTINITIALIZED: HResult = 0x8898_2f0c_u32 as i32;
pub const WINCODEC_ERR_ALREADYLOCKED: HResult = 0x8898_2f0d_u32 as i32;
pub const WINCODEC_ERR_CODECNOTHUMBNAIL: HResult = 0x8898_2f44_u32 as i32;
pub const WINCODEC_ERR_PALETTEUNAVAILABLE: HResult = 0x8898_2f45_u32 as i32;
pub const WINCODEC_ERR_COMPONENTNOTFOUND: HResult = 0x8898_2f50_u32 as i32;
pub const WINCODEC_ERR_BADIMAGE: HResult = 0x8898_2f60_u32 as i32;
pub const WINCODEC_ERR_UNSUPPORTEDPIXELFORMAT: HResult = 0x8898_2f80_u32 as i32;
pub const WINCODEC_ERR_UNSUPPORTEDOPERATION: HResult = 0x8898_2f81_u32 as i32;
pub const WINCODEC_ERR_INSUFFICIENTBUFFER: HResult = 0x8898_2f8c_u32 as i32;

/// The ledger's kinds (GalaRecordMissing's codes).
const LEDGER_UNSUPPORTED: u32 = 2;
const LEDGER_NOT_IMPLEMENTED: u32 = 3;
const LEDGER_OTHER: u32 = 5;

/// The ledger names of png.rs's limits.
pub(crate) const PNG_GAMA: &[u8] = b"windowscodecs.dll!png(gAMA not applied)";
pub(crate) const PNG_INTERLACED: &[u8] = b"windowscodecs.dll!png(interlaced)";

/// Records `name` (a whole ledger name) as unsupported (png.rs's hook).
pub(crate) fn record(name: &[u8], detail: u64) {
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(name.as_ptr(), name.len(), LEDGER_UNSUPPORTED, detail) };
}

fn record_kind(what: &[u8], kind: u32, detail: u64) {
    let mut name = [0u8; 96];
    let mut n = 0;
    for &b in b"windowscodecs.dll!".iter().chain(what) {
        if n == name.len() {
            break;
        }
        name[n] = b;
        n += 1;
    }
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(name.as_ptr(), n, kind, detail) };
}

/// Records `windowscodecs.dll!<what>`: something this DLL does not
/// support (a format, an option), with a detail.
pub fn gap(what: &[u8], detail: u64) {
    record_kind(what, LEDGER_UNSUPPORTED, detail);
}

/// A method gala does not implement: recorded, E_NOTIMPL.
pub fn not_implemented(method: &[u8]) -> HResult {
    record_kind(method, LEDGER_NOT_IMPLEMENTED, 0);
    E_NOTIMPL
}

/// The first dword of a GUID (a ledger detail).
pub fn data1(g: &Guid) -> u64 {
    u64::from(u32::from_le_bytes([g[0], g[1], g[2], g[3]]))
}

/// The bytes of a GUID given as DEFINE_GUID does.
pub const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

pub const IID_IUNKNOWN: Guid = guid(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
/// The WIC interfaces whose IIDs share their last eight bytes.
const WIC_D4: [u8; 8] = [0xba, 0x0a, 0xfd, 0x2b, 0x66, 0x45, 0xfb, 0x94];
pub const IID_IWICBITMAPSOURCE: Guid = guid(0x0000_0120, 0xa8f2, 0x4877, WIC_D4);
pub const IID_IWICBITMAP: Guid = guid(0x0000_0121, 0xa8f2, 0x4877, WIC_D4);
pub const IID_IWICBITMAPLOCK: Guid = guid(0x0000_0123, 0xa8f2, 0x4877, WIC_D4);
pub const IID_IWICPALETTE: Guid = guid(0x0000_0040, 0xa8f2, 0x4877, WIC_D4);
pub const IID_IWICFORMATCONVERTER: Guid = guid(0x0000_0301, 0xa8f2, 0x4877, WIC_D4);
pub const IID_IWICBITMAPSCALER: Guid = guid(0x0000_0302, 0xa8f2, 0x4877, WIC_D4);

/// Reads the caller's GUID (None for NULL).
pub fn read_guid(g: *const Guid) -> Option<Guid> {
    if g.is_null() {
        None
    } else {
        // SAFETY: the caller's GUID.
        Some(unsafe { g.read_unaligned() })
    }
}

/// Stores `v` at the caller's output, if any.
pub fn put<T>(out: *mut T, v: T) {
    if !out.is_null() {
        // SAFETY: the caller's output.
        unsafe { out.write_unaligned(v) };
    }
}

/// A new heap object holding `v` (None when out of memory).
pub fn try_box<T>(v: T) -> Option<*mut T> {
    let layout = core::alloc::Layout::new::<T>();
    // SAFETY: a non-zero-sized layout (every object has a header).
    let p = unsafe { alloc::alloc::alloc(layout) }.cast::<T>();
    if p.is_null() {
        return None;
    }
    // SAFETY: fresh memory for one T.
    unsafe { p.write(v) };
    Some(p)
}

/// Frees an object from `try_box`.
///
/// # Safety
/// `p` came from `try_box::<T>` and is not used after.
pub unsafe fn free_box<T>(p: *mut T) {
    // SAFETY: as the caller promises.
    unsafe {
        core::ptr::drop_in_place(p);
        alloc::alloc::dealloc(p.cast(), core::alloc::Layout::new::<T>());
    }
}

/// A zeroed byte buffer of `n` bytes (None when out of memory).
pub fn zeroed(n: usize) -> Option<alloc::vec::Vec<u8>> {
    let mut v = alloc::vec::Vec::new();
    v.try_reserve_exact(n).ok()?;
    v.resize(n, 0);
    Some(v)
}

/// A vtable: the methods' addresses, in the interface's order.
#[repr(C)]
pub struct VTable<const N: usize>(pub [*const (); N]);
// SAFETY: function addresses only, never written.
unsafe impl<const N: usize> Sync for VTable<N> {}

impl<const N: usize> VTable<N> {
    pub const fn addr(&'static self) -> *const () {
        (self as *const Self).cast()
    }
}

/// What every object of this DLL starts with: its vtable and its
/// references.
#[repr(C)]
pub struct Header {
    pub vtbl: *const (),
    pub refs: AtomicU32,
}
// SAFETY: an immutable vtable pointer and an atomic.
unsafe impl Sync for Header {}

impl Header {
    pub const fn new(vtbl: *const ()) -> Self {
        Header { vtbl, refs: AtomicU32::new(1) }
    }
}

/// The object's vtable (to recognise this DLL's own objects).
pub fn vtable_of(o: Obj) -> *const () {
    // SAFETY: every COM object starts with its vtable pointer.
    unsafe { *(o as *const *const ()) }
}

/// IUnknown::AddRef of this DLL's objects.
pub extern "system" fn add_ref(this: Obj) -> u32 {
    // SAFETY: one of our objects: it starts with a Header.
    unsafe { (*(this as *const Header)).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

/// Drops one reference of object `this` of type `T`, freeing it at zero.
pub fn release_object<T>(this: Obj) -> u32 {
    // SAFETY: one of our objects of type T: it starts with a Header.
    let left = unsafe { (*(this as *const Header)).refs.fetch_sub(1, Ordering::AcqRel) } - 1;
    if left == 0 {
        // SAFETY: the last reference is gone; it came from try_box::<T>.
        unsafe { free_box(this as *mut T) };
    }
    left
}

/// QueryInterface of an object answering `iids` and IUnknown.
pub fn query(this: Obj, iids: &[Guid], riid: *const Guid, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let Some(iid) = read_guid(riid) else {
        put(out, core::ptr::null_mut());
        return E_INVALIDARG;
    };
    if iid == IID_IUNKNOWN || iids.contains(&iid) {
        add_ref(this);
        put(out, this);
        S_OK
    } else {
        put(out, core::ptr::null_mut());
        E_NOINTERFACE
    }
}

/// A small spin lock for an object's mutable state. Never held across a
/// call into another object (which might call back).
pub struct Spin<T> {
    busy: AtomicBool,
    value: UnsafeCell<T>,
}
// SAFETY: the value is only reached while `busy` is held.
unsafe impl<T: Send> Sync for Spin<T> {}
// SAFETY: as above.
unsafe impl<T: Send> Send for Spin<T> {}

impl<T> Spin<T> {
    pub const fn new(v: T) -> Self {
        Spin { busy: AtomicBool::new(false), value: UnsafeCell::new(v) }
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        while self
            .busy
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        // SAFETY: `busy` is ours until the store below.
        let r = f(unsafe { &mut *self.value.get() });
        self.busy.store(false, Ordering::Release);
        r
    }
}

/// WICRect.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// The caller's rectangle within a `w` x `h` source (NULL: all of it),
/// as (x, y, width, height); E_INVALIDARG when it leaves the source or
/// is empty.
pub fn resolve_rect(rect: *const Rect, w: u32, h: u32) -> Result<(u32, u32, u32, u32), HResult> {
    if rect.is_null() {
        return Ok((0, 0, w, h));
    }
    // SAFETY: the caller's rectangle.
    let r = unsafe { rect.read_unaligned() };
    if r.x < 0 || r.y < 0 || r.w <= 0 || r.h <= 0 {
        return Err(E_INVALIDARG);
    }
    let (x, y, rw, rh) = (r.x as u32, r.y as u32, r.w as u32, r.h as u32);
    if u64::from(x) + u64::from(rw) > u64::from(w) || u64::from(y) + u64::from(rh) > u64::from(h) {
        return Err(E_INVALIDARG);
    }
    Ok((x, y, rw, rh))
}

/// The bytes of `w` pixels of `bpp` bits.
pub fn row_bytes(w: u32, bpp: u32) -> usize {
    (w as usize * bpp as usize).div_ceil(8)
}

/// The caller's output buffer for `rh` rows of `row` bytes each, `stride`
/// apart, checked: E_INVALIDARG for a NULL buffer, a stride below a row
/// or a size below the rows.
pub fn out_buffer<'a>(
    row: usize,
    rh: u32,
    stride: u32,
    size: u32,
    buf: *mut u8,
) -> Result<&'a mut [u8], HResult> {
    let need = stride as usize * (rh as usize - 1) + row;
    if buf.is_null() || (stride as usize) < row || (size as usize) < need {
        return Err(E_INVALIDARG);
    }
    // SAFETY: the caller's buffer of `size` bytes.
    Ok(unsafe { core::slice::from_raw_parts_mut(buf, need) })
}

/// The `bpp`-bit field of pixel `x` in `line` (high bits first).
pub fn get_bits(line: &[u8], x: usize, bpp: u32) -> u32 {
    let bit = x * bpp as usize;
    let shift = 8 - bpp as usize - bit % 8;
    u32::from((line[bit / 8] >> shift) & ((1u16 << bpp) - 1) as u8)
}

/// Sets the `bpp`-bit field of pixel `x` in `line` to `v`.
pub fn put_bits(line: &mut [u8], x: usize, bpp: u32, v: u32) {
    let bit = x * bpp as usize;
    let shift = 8 - bpp as usize - bit % 8;
    let mask = (((1u16 << bpp) - 1) as u8) << shift;
    line[bit / 8] = (line[bit / 8] & !mask) | (((v as u8) << shift) & mask);
}

/// CopyPixels of a source held in memory (`data`, rows `stride` apart,
/// `w` x `h` pixels of `bpp` bits): the caller's rectangle into the
/// caller's buffer.
#[allow(clippy::too_many_arguments)]
pub fn copy_from_memory(
    data: &[u8],
    data_stride: usize,
    bpp: u32,
    w: u32,
    h: u32,
    rect: *const Rect,
    stride: u32,
    size: u32,
    buf: *mut u8,
) -> HResult {
    let (x, y, rw, rh) = match resolve_rect(rect, w, h) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let row = row_bytes(rw, bpp);
    let out = match out_buffer(row, rh, stride, size, buf) {
        Ok(o) => o,
        Err(e) => return e,
    };
    for j in 0..rh as usize {
        let src = &data[(y as usize + j) * data_stride..];
        let dst = &mut out[j * stride as usize..j * stride as usize + row];
        if (x as usize * bpp as usize).is_multiple_of(8) {
            let start = x as usize * bpp as usize / 8;
            dst.copy_from_slice(&src[start..start + row]);
        } else {
            for i in 0..rw as usize {
                put_bits(dst, i, bpp, get_bits(src, x as usize + i, bpp));
            }
        }
    }
    S_OK
}

/// Any IWICBitmapSource, this DLL's or the caller's, called through its
/// vtable (GetSize, GetPixelFormat, GetResolution, CopyPalette and
/// CopyPixels are slots 3 to 7).
#[derive(Clone, Copy)]
pub struct Source(pub Obj);

type FnUnknown = extern "system" fn(Obj) -> u32;
type FnSize = extern "system" fn(Obj, *mut u32, *mut u32) -> HResult;
type FnFormat = extern "system" fn(Obj, *mut Guid) -> HResult;
type FnResolution = extern "system" fn(Obj, *mut f64, *mut f64) -> HResult;
type FnPalette = extern "system" fn(Obj, Obj) -> HResult;
type FnCopy = extern "system" fn(Obj, *const Rect, u32, u32, *mut u8) -> HResult;

/// Slot `i` of COM object `o`'s vtable.
pub fn slot(o: Obj, i: usize) -> *const () {
    // SAFETY: a COM object: its first field points at its vtable, which
    // has at least the slots of the interface it was given as.
    unsafe { *(*(o as *const *const *const ())).add(i) }
}

impl Source {
    pub fn add_ref(self) {
        // SAFETY: IUnknown::AddRef's signature.
        let f: FnUnknown = unsafe { core::mem::transmute(slot(self.0, 1)) };
        f(self.0);
    }

    pub fn release(self) {
        // SAFETY: IUnknown::Release's signature.
        let f: FnUnknown = unsafe { core::mem::transmute(slot(self.0, 2)) };
        f(self.0);
    }

    pub fn size(self) -> Result<(u32, u32), HResult> {
        let (mut w, mut h) = (0, 0);
        // SAFETY: IWICBitmapSource::GetSize's signature.
        let f: FnSize = unsafe { core::mem::transmute(slot(self.0, 3)) };
        match f(self.0, &mut w, &mut h) {
            S_OK => Ok((w, h)),
            e => Err(e),
        }
    }

    pub fn format(self) -> Result<Guid, HResult> {
        let mut g = [0u8; 16];
        // SAFETY: IWICBitmapSource::GetPixelFormat's signature.
        let f: FnFormat = unsafe { core::mem::transmute(slot(self.0, 4)) };
        match f(self.0, &mut g) {
            S_OK => Ok(g),
            e => Err(e),
        }
    }

    pub fn resolution(self) -> Result<(f64, f64), HResult> {
        let (mut x, mut y) = (0.0, 0.0);
        // SAFETY: IWICBitmapSource::GetResolution's signature.
        let f: FnResolution = unsafe { core::mem::transmute(slot(self.0, 5)) };
        match f(self.0, &mut x, &mut y) {
            S_OK => Ok((x, y)),
            e => Err(e),
        }
    }

    pub fn copy_palette(self, palette: Obj) -> HResult {
        // SAFETY: IWICBitmapSource::CopyPalette's signature.
        let f: FnPalette = unsafe { core::mem::transmute(slot(self.0, 6)) };
        f(self.0, palette)
    }

    pub fn copy_pixels(self, rect: *const Rect, stride: u32, size: u32, buf: *mut u8) -> HResult {
        // SAFETY: IWICBitmapSource::CopyPixels's signature.
        let f: FnCopy = unsafe { core::mem::transmute(slot(self.0, 7)) };
        f(self.0, rect, stride, size, buf)
    }

    /// The source's palette colours (None when it has none).
    pub fn palette_colors(self) -> Option<alloc::vec::Vec<u32>> {
        let p = palette::new_palette()?;
        let r = self.copy_palette(p);
        let colors = if r == S_OK { palette::colors(p).map(|(_, c)| c) } else { None };
        release_object::<palette::Palette>(p);
        colors
    }
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

/// A panic is a bug: its location goes to the ledger
/// (`windowscodecs.dll!panic <file>`, the line as detail) and the process
/// ends with STATUS_STACK_BUFFER_OVERRUN (fail-fast), as gala's other
/// DLLs do.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut text = [0u8; 72];
    let mut n = 0;
    let line = info.location().map_or(0, |l| {
        for &b in b"panic ".iter().chain(l.file().as_bytes()) {
            if n < text.len() {
                text[n] = b;
                n += 1;
            }
        }
        l.line()
    });
    record_kind(&text[..n], LEDGER_OTHER, line.into());
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
