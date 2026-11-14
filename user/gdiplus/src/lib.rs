//! gala's gdiplus.dll (GDI+'s flat API, gdiplusflat.h): the part the
//! Office Deployment Tool's setup.exe draws its window with (ROADMAP OF;
//! it delay-loads these seventeen functions when its configuration shows
//! a window, Display Level Full): startup, bitmaps over the caller's
//! memory or decoded from a PNG stream, solid brushes, and graphics on a
//! bitmap or a device context that fill rectangles and draw images.
//!
//! What Microsoft's documentation states, checked by `gdiplus_test` and
//! its x86 twin `wow64_gdiplus_test`:
//! - GdiplusStartup takes GdiplusStartupInput versions 1 to 3; another
//!   version is UnsupportedGdiplusVersion. With SuppressBackgroundThread
//!   the caller gets the notification hook and unhook functions.
//! - GdipCreateBitmapFromScan0: width and height above 0; with a buffer,
//!   a stride that is a multiple of four (negative: a bottom-up buffer),
//!   which the bitmap uses as its pixels; without one, GDI+'s own zeroed
//!   memory. 32 bpp ARGB, PARGB and RGB and 24 bpp RGB are kept here;
//!   other formats are NotImplemented, recorded
//!   (`gdiplus.dll!GdipCreateBitmapFromScan0(pixel format)`).
//! - GdipLoadImageFromStream reads the stream from its start; PNG is
//!   decoded (`png`), as 32 bpp ARGB; other formats are NotImplemented,
//!   recorded (`gdiplus.dll!GdipLoadImageFromStream(...)`).
//! - Drawing composes source over destination (CompositingModeSourceOver,
//!   the default), in premultiplied terms: dst = src + dst * (1 - srcA).
//!   An image drawn at its own size is copied pixel for pixel; a scaled
//!   one is sampled bilinearly (InterpolationModeBilinear, the default),
//!   its edges clamped. On a device context the pixels go through gdi32's
//!   GdiAlphaBlend from a 32 bpp DIB section.
//! - GdipCreateFromHDC(NULL) is OutOfMemory (Wine's graphics.c, run on
//!   Windows); a NULL object elsewhere is InvalidParameter.
//!
//! Not here (each recorded where reached): pens, paths, text, transforms,
//! clipping, other compositing and interpolation modes, codecs other than
//! PNG's decoder, encoders.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;

mod png;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};

use gala_heap as _;
#[cfg(target_arch = "x86")]
use ntrt as _;

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

// The x64 build's memory routines come from gala's ntdll (as other
// DLLs on `alloc` take them).
#[cfg(target_arch = "x86_64")]
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

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "gdi32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "gdi32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CreateCompatibleDC(hdc: usize) -> usize;
    fn DeleteDC(hdc: usize) -> i32;
    fn CreateDIBSection(
        hdc: usize,
        info: *const BitmapInfoHeader,
        usage: u32,
        bits: *mut *mut u8,
        section: usize,
        offset: u32,
    ) -> usize;
    fn SelectObject(hdc: usize, object: usize) -> usize;
    fn DeleteObject(object: usize) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn GdiAlphaBlend(
        dst: usize,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        src: usize,
        sx: i32,
        sy: i32,
        sw: i32,
        sh: i32,
        blend: u32,
    ) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut c_void;
    fn HeapFree(heap: usize, flags: u32, p: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
}

/// GpStatus.
pub type Status = u32;
const OK: Status = 0;
const GENERIC_ERROR: Status = 1;
const INVALID_PARAMETER: Status = 2;
const OUT_OF_MEMORY: Status = 3;
const NOT_IMPLEMENTED: Status = 6;
const WIN32_ERROR: Status = 7;
const UNSUPPORTED_GDIPLUS_VERSION: Status = 17;

/// PixelFormat values (gdipluspixelformats.h).
const PIXEL_FORMAT_24BPP_RGB: u32 = 0x0002_1808;
const PIXEL_FORMAT_32BPP_RGB: u32 = 0x0002_2009;
const PIXEL_FORMAT_32BPP_ARGB: u32 = 0x0026_200a;
const PIXEL_FORMAT_32BPP_PARGB: u32 = 0x000e_200b;

/// The ledger's kinds (GalaRecordMissing's codes).
const LEDGER_UNSUPPORTED: u32 = 2;
const LEDGER_OTHER: u32 = 5;

/// The ledger names of png.rs's limits.
pub(crate) const PNG_GAMA: &[u8] = b"gdiplus.dll!png(gAMA not applied)";
pub(crate) const PNG_INTERLACED: &[u8] = b"gdiplus.dll!png(interlaced)";

pub(crate) fn record(what: &[u8], detail: u64) {
    // SAFETY: a valid byte string.
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), LEDGER_UNSUPPORTED, detail) };
}

/// Tags that tell gala's objects apart (GDI+ hands out plain pointers):
/// each object is `repr(C)` with its tag first.
const IMAGE_TAG: u32 = 0x4750_696d; // "GPim"
const BRUSH_TAG: u32 = 0x4750_6272; // "GPbr"
const GRAPHICS_TAG: u32 = 0x4750_6772; // "GPgr"

/// A bitmap (GpBitmap, also a GpImage).
#[repr(C)]
pub struct Image {
    tag: u32,
    width: u32,
    height: u32,
    format: u32,
    /// Bytes from one row to the next (negative: bottom-up memory).
    stride: isize,
    /// The first (top) row.
    scan0: *mut u8,
    /// GDI+'s own pixels (`scan0` points into them); empty when they
    /// are the caller's. Held only to keep them alive.
    _owned: Vec<u8>,
}

/// A solid brush (GpSolidFill, also a GpBrush).
#[repr(C)]
pub struct Brush {
    tag: u32,
    argb: u32,
}

enum Target {
    Image(*mut Image),
    Hdc(usize),
}

/// A GpGraphics: what it draws on.
#[repr(C)]
pub struct Graphics {
    tag: u32,
    target: Target,
}

fn bytes_per_pixel(format: u32) -> Option<usize> {
    match format {
        PIXEL_FORMAT_24BPP_RGB => Some(3),
        PIXEL_FORMAT_32BPP_RGB | PIXEL_FORMAT_32BPP_ARGB | PIXEL_FORMAT_32BPP_PARGB => Some(4),
        _ => None,
    }
}

/// A zeroed buffer of `n` bytes, or None when the heap has no room.
fn zeroed(n: usize) -> Option<Vec<u8>> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).ok()?;
    v.resize(n, 0);
    Some(v)
}

/// Premultiplied 0xAARRGGBB from straight.
fn premultiply(argb: u32) -> u32 {
    let a = argb >> 24;
    if a == 255 {
        return argb;
    }
    let mul = |shift: u32| (((argb >> shift) & 0xff) * a + 127) / 255;
    a << 24 | mul(16) << 16 | mul(8) << 8 | mul(0)
}

/// Straight 0xAARRGGBB from premultiplied.
fn unpremultiply(p: u32) -> u32 {
    let a = p >> 24;
    match a {
        255 => p,
        0 => 0,
        _ => {
            let div = |shift: u32| ((((p >> shift) & 0xff) * 255 + a / 2) / a).min(255);
            a << 24 | div(16) << 16 | div(8) << 8 | div(0)
        }
    }
}

/// Source over destination, both premultiplied.
fn over(s: u32, d: u32) -> u32 {
    let sa = s >> 24;
    if sa == 255 {
        return s;
    }
    if sa == 0 {
        return d;
    }
    let mut out = 0;
    for shift in [0, 8, 16, 24] {
        let sv = (s >> shift) & 0xff;
        let dv = (d >> shift) & 0xff;
        let v = sv + (dv * (255 - sa) + 127) / 255;
        out |= v.min(255) << shift;
    }
    out
}

impl Image {
    fn new(width: u32, height: u32, format: u32) -> Option<Self> {
        let bpp = bytes_per_pixel(format)?;
        let stride = (width as usize * bpp).next_multiple_of(4);
        let mut owned = zeroed(stride.checked_mul(height as usize)?)?;
        let scan0 = owned.as_mut_ptr();
        Some(Self {
            tag: IMAGE_TAG,
            width,
            height,
            format,
            stride: stride as isize,
            scan0,
            _owned: owned,
        })
    }

    fn at(&self, x: u32, y: u32) -> *mut u8 {
        let bpp = bytes_per_pixel(self.format).unwrap_or(4);
        // SAFETY (for callers): x < width and y < height keep the offset
        // within the rows the bitmap was made with.
        self.scan0.wrapping_offset(y as isize * self.stride + (x as usize * bpp) as isize)
    }

    /// Pixel (x, y), premultiplied.
    fn get(&self, x: u32, y: u32) -> u32 {
        let p = self.at(x, y);
        // SAFETY: a pixel of the bitmap's memory (x, y within its size).
        unsafe {
            match self.format {
                PIXEL_FORMAT_24BPP_RGB => {
                    0xff00_0000
                        | u32::from(*p.add(2)) << 16
                        | u32::from(*p.add(1)) << 8
                        | u32::from(*p)
                }
                PIXEL_FORMAT_32BPP_RGB => 0xff00_0000 | p.cast::<u32>().read_unaligned(),
                PIXEL_FORMAT_32BPP_PARGB => p.cast::<u32>().read_unaligned(),
                _ => premultiply(p.cast::<u32>().read_unaligned()),
            }
        }
    }

    /// Stores premultiplied `v` at (x, y).
    fn put(&mut self, x: u32, y: u32, v: u32) {
        let p = self.at(x, y);
        // SAFETY: as in `get`.
        unsafe {
            match self.format {
                PIXEL_FORMAT_24BPP_RGB => {
                    let s = unpremultiply(v | 0xff00_0000);
                    *p = s as u8;
                    *p.add(1) = (s >> 8) as u8;
                    *p.add(2) = (s >> 16) as u8;
                }
                PIXEL_FORMAT_32BPP_RGB => {
                    p.cast::<u32>().write_unaligned(0xff00_0000 | (v & 0x00ff_ffff));
                }
                PIXEL_FORMAT_32BPP_PARGB => p.cast::<u32>().write_unaligned(v),
                _ => p.cast::<u32>().write_unaligned(unpremultiply(v)),
            }
        }
    }

    /// A copy with pixels of its own.
    fn duplicate(&self) -> Option<Self> {
        let copy = Self::new(self.width, self.height, self.format)?;
        let bpp = bytes_per_pixel(self.format)?;
        for y in 0..self.height {
            // SAFETY: one row of each bitmap, `width * bpp` bytes.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.at(0, y),
                    copy.at(0, y),
                    self.width as usize * bpp,
                );
            }
        }
        Some(copy)
    }

    /// Premultiplied sample at (fx, fy) in source pixels (centres at
    /// .5), bilinear with the edges clamped.
    fn sample(&self, fx: f32, fy: f32) -> u32 {
        let clamp = |v: f32, max: u32| v.max(0.0).min((max - 1) as f32);
        let (fx, fy) = (clamp(fx - 0.5, self.width), clamp(fy - 0.5, self.height));
        let (x0, y0) = (fx as u32, fy as u32);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let (a, b, c, d) = (self.get(x0, y0), self.get(x1, y0), self.get(x0, y1), self.get(x1, y1));
        let mut out = 0;
        for shift in [0, 8, 16, 24] {
            let ch = |p: u32| ((p >> shift) & 0xff) as f32;
            let top = ch(a) + (ch(b) - ch(a)) * tx;
            let bottom = ch(c) + (ch(d) - ch(c)) * tx;
            let v = top + (bottom - top) * ty + 0.5;
            out |= (v as u32).min(255) << shift;
        }
        out
    }
}

/// `p` as a live object of type T with tag `tag`, or None.
fn object<'a, T>(p: *mut T, tag: u32) -> Option<&'a mut T> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the caller passes a pointer this DLL handed out (GDI+
    // trusts its callers the same way); every object starts with its
    // u32 tag, checked before the rest is used.
    unsafe { (p.cast::<u32>().read() == tag).then(|| &mut *p) }
}

fn image<'a>(p: *mut Image) -> Option<&'a mut Image> {
    object(p, IMAGE_TAG)
}

/// Hands `value` out as a GDI+ object pointer through `out`.
fn hand_out<T>(value: T, out: *mut *mut T) -> Status {
    let Some(boxed) = try_box(value) else { return OUT_OF_MEMORY };
    // SAFETY: `out` checked non-null by the callers.
    unsafe { *out = Box::into_raw(boxed) };
    OK
}

fn try_box<T>(value: T) -> Option<Box<T>> {
    // Box::new aborts on a full heap; GDI+ answers OutOfMemory.
    let layout = core::alloc::Layout::new::<T>();
    // SAFETY: a non-zero-sized layout (the objects all have fields);
    // the block is initialized before the Box owns it.
    unsafe {
        let p = alloc::alloc::alloc(layout).cast::<T>();
        if p.is_null() {
            return None;
        }
        p.write(value);
        Some(Box::from_raw(p))
    }
}

// ---- Startup ----

/// GdiplusStartupInput.
#[repr(C)]
pub struct StartupInput {
    version: u32,
    debug_callback: usize,
    suppress_background_thread: i32,
    suppress_external_codecs: i32,
}

/// GdiplusStartupOutput.
#[repr(C)]
pub struct StartupOutput {
    hook: usize,
    unhook: usize,
}

static TOKENS: AtomicUsize = AtomicUsize::new(0);

extern "system" fn notification_hook(token: *mut usize) -> Status {
    if token.is_null() {
        return INVALID_PARAMETER;
    }
    // SAFETY: the caller's out-parameter.
    unsafe { *token = TOKENS.fetch_add(1, Ordering::Relaxed) + 1 };
    OK
}

extern "system" fn notification_unhook(_token: usize) {}

/// GdiplusStartup(token, input, output).
#[unsafe(no_mangle)]
pub extern "system" fn GdiplusStartup(
    token: *mut usize,
    input: *const StartupInput,
    output: *mut StartupOutput,
) -> Status {
    if token.is_null() || input.is_null() {
        return INVALID_PARAMETER;
    }
    // SAFETY: the caller's GdiplusStartupInput.
    let input = unsafe { &*input };
    if !(1..=3).contains(&input.version) {
        return UNSUPPORTED_GDIPLUS_VERSION;
    }
    if input.suppress_background_thread != 0 {
        if output.is_null() {
            return INVALID_PARAMETER;
        }
        // SAFETY: the caller's GdiplusStartupOutput.
        unsafe {
            (*output).hook = notification_hook as *const () as usize;
            (*output).unhook = notification_unhook as *const () as usize;
        }
    }
    // SAFETY: checked non-null.
    unsafe { *token = TOKENS.fetch_add(1, Ordering::Relaxed) + 1 };
    OK
}

/// GdiplusShutdown(token): nothing to stop (no background thread).
#[unsafe(no_mangle)]
pub extern "system" fn GdiplusShutdown(_token: usize) {}

/// GdipAlloc(size): GDI+'s heap (the process heap).
#[unsafe(no_mangle)]
pub extern "system" fn GdipAlloc(size: usize) -> *mut c_void {
    // SAFETY: plain heap calls.
    unsafe { HeapAlloc(GetProcessHeap(), 0, size) }
}

/// GdipFree(p).
#[unsafe(no_mangle)]
pub extern "system" fn GdipFree(p: *mut c_void) {
    if !p.is_null() {
        // SAFETY: a block from GdipAlloc.
        unsafe { HeapFree(GetProcessHeap(), 0, p) };
    }
}

// ---- Images ----

/// GdipCreateBitmapFromScan0(width, height, stride, format, scan0, bitmap).
#[unsafe(no_mangle)]
pub extern "system" fn GdipCreateBitmapFromScan0(
    width: i32,
    height: i32,
    stride: i32,
    format: u32,
    scan0: *mut u8,
    bitmap: *mut *mut Image,
) -> Status {
    if bitmap.is_null() {
        return INVALID_PARAMETER;
    }
    // SAFETY: the caller's out-parameter.
    unsafe { *bitmap = core::ptr::null_mut() };
    if width <= 0 || height <= 0 || (!scan0.is_null() && (stride == 0 || stride % 4 != 0)) {
        return INVALID_PARAMETER;
    }
    if bytes_per_pixel(format).is_none() {
        record(b"gdiplus.dll!GdipCreateBitmapFromScan0(pixel format)", format.into());
        return NOT_IMPLEMENTED;
    }
    let image = if scan0.is_null() {
        match Image::new(width as u32, height as u32, format) {
            Some(i) => i,
            None => return OUT_OF_MEMORY,
        }
    } else {
        Image {
            tag: IMAGE_TAG,
            width: width as u32,
            height: height as u32,
            format,
            stride: stride as isize,
            scan0,
            _owned: Vec::new(),
        }
    };
    hand_out(image, bitmap)
}

/// GdipGetImageWidth(image, width).
#[unsafe(no_mangle)]
pub extern "system" fn GdipGetImageWidth(image_: *mut Image, width: *mut u32) -> Status {
    match image(image_) {
        Some(i) if !width.is_null() => {
            // SAFETY: the caller's out-parameter.
            unsafe { *width = i.width };
            OK
        }
        _ => INVALID_PARAMETER,
    }
}

/// GdipGetImageHeight(image, height).
#[unsafe(no_mangle)]
pub extern "system" fn GdipGetImageHeight(image_: *mut Image, height: *mut u32) -> Status {
    match image(image_) {
        Some(i) if !height.is_null() => {
            // SAFETY: the caller's out-parameter.
            unsafe { *height = i.height };
            OK
        }
        _ => INVALID_PARAMETER,
    }
}

/// GdipCloneImage(image, clone): a copy with pixels of its own.
#[unsafe(no_mangle)]
pub extern "system" fn GdipCloneImage(image_: *mut Image, clone: *mut *mut Image) -> Status {
    let Some(i) = image(image_) else { return INVALID_PARAMETER };
    if clone.is_null() {
        return INVALID_PARAMETER;
    }
    match i.duplicate() {
        Some(copy) => hand_out(copy, clone),
        None => OUT_OF_MEMORY,
    }
}

/// GdipDisposeImage(image).
#[unsafe(no_mangle)]
pub extern "system" fn GdipDisposeImage(image_: *mut Image) -> Status {
    let Some(i) = image(image_) else { return INVALID_PARAMETER };
    i.tag = 0;
    // SAFETY: a Box this DLL handed out (`hand_out`).
    drop(unsafe { Box::from_raw(image_) });
    OK
}

/// IStream's methods this DLL calls (objidl.h's vtable order).
#[repr(C)]
struct StreamVtbl {
    query_interface: usize,
    add_ref: usize,
    release: usize,
    read: unsafe extern "system" fn(*mut c_void, *mut u8, u32, *mut u32) -> i32,
    write: usize,
    seek: unsafe extern "system" fn(*mut c_void, i64, u32, *mut u64) -> i32,
}

/// The most a stream may hold for an image (a guard against a stream
/// that never ends).
const MAX_STREAM: usize = 256 << 20;

/// GdipLoadImageFromStream(stream, image): the stream from its start.
#[unsafe(no_mangle)]
pub extern "system" fn GdipLoadImageFromStream(
    stream: *mut c_void,
    out: *mut *mut Image,
) -> Status {
    if stream.is_null() || out.is_null() {
        return INVALID_PARAMETER;
    }
    // SAFETY: the caller's IStream: its first field is its vtable.
    let vtbl = unsafe { &**stream.cast::<*const StreamVtbl>() };
    // SAFETY: IStream::Seek(STREAM_SEEK_SET, 0) and IStream::Read into
    // our buffer's spare room.
    let data = unsafe {
        if (vtbl.seek)(stream, 0, 0, core::ptr::null_mut()) < 0 {
            return WIN32_ERROR;
        }
        let mut data: Vec<u8> = Vec::new();
        loop {
            if data.try_reserve(64 << 10).is_err() {
                return OUT_OF_MEMORY;
            }
            let room = (data.capacity() - data.len()).min(u32::MAX as usize) as u32;
            let mut got = 0u32;
            let hr = (vtbl.read)(stream, data.as_mut_ptr().add(data.len()), room, &mut got);
            if hr < 0 {
                return WIN32_ERROR;
            }
            data.set_len(data.len() + got as usize);
            if got == 0 || data.len() > MAX_STREAM {
                break;
            }
        }
        data
    };
    match png::decode(&data) {
        Ok(decoded) => {
            let Some(img) = Image::new(decoded.width, decoded.height, PIXEL_FORMAT_32BPP_ARGB)
            else {
                return OUT_OF_MEMORY;
            };
            for y in 0..decoded.height {
                for x in 0..decoded.width {
                    let v = decoded.pixels[(y * decoded.width + x) as usize];
                    // SAFETY: a pixel within the new bitmap.
                    unsafe { img.at(x, y).cast::<u32>().write_unaligned(v) };
                }
            }
            hand_out(img, out)
        }
        Err(png::Error::OutOfMemory) => OUT_OF_MEMORY,
        Err(png::Error::NotPng) => {
            record(b"gdiplus.dll!GdipLoadImageFromStream(not a PNG)", data.len() as u64);
            NOT_IMPLEMENTED
        }
        Err(png::Error::Unsupported(what)) => {
            record(what, 0);
            NOT_IMPLEMENTED
        }
        Err(png::Error::Corrupt) => GENERIC_ERROR,
    }
}

// ---- Brushes ----

/// GdipCreateSolidFill(argb, brush).
#[unsafe(no_mangle)]
pub extern "system" fn GdipCreateSolidFill(argb: u32, brush: *mut *mut Brush) -> Status {
    if brush.is_null() {
        return INVALID_PARAMETER;
    }
    hand_out(Brush { tag: BRUSH_TAG, argb }, brush)
}

/// GdipCloneBrush(brush, clone).
#[unsafe(no_mangle)]
pub extern "system" fn GdipCloneBrush(brush: *mut Brush, clone: *mut *mut Brush) -> Status {
    match object(brush, BRUSH_TAG) {
        Some(b) if !clone.is_null() => hand_out(Brush { tag: BRUSH_TAG, argb: b.argb }, clone),
        _ => INVALID_PARAMETER,
    }
}

/// GdipDeleteBrush(brush).
#[unsafe(no_mangle)]
pub extern "system" fn GdipDeleteBrush(brush: *mut Brush) -> Status {
    let Some(b) = object(brush, BRUSH_TAG) else { return INVALID_PARAMETER };
    b.tag = 0;
    // SAFETY: a Box this DLL handed out.
    drop(unsafe { Box::from_raw(brush) });
    OK
}

// ---- Graphics ----

/// GdipCreateFromHDC(hdc, graphics): drawing on a device context.
#[unsafe(no_mangle)]
pub extern "system" fn GdipCreateFromHDC(hdc: usize, graphics: *mut *mut Graphics) -> Status {
    if graphics.is_null() {
        return INVALID_PARAMETER;
    }
    if hdc == 0 {
        return OUT_OF_MEMORY;
    }
    hand_out(Graphics { tag: GRAPHICS_TAG, target: Target::Hdc(hdc) }, graphics)
}

/// GdipGetImageGraphicsContext(image, graphics): drawing on a bitmap.
#[unsafe(no_mangle)]
pub extern "system" fn GdipGetImageGraphicsContext(
    image_: *mut Image,
    graphics: *mut *mut Graphics,
) -> Status {
    if image(image_).is_none() || graphics.is_null() {
        return INVALID_PARAMETER;
    }
    hand_out(Graphics { tag: GRAPHICS_TAG, target: Target::Image(image_) }, graphics)
}

/// GdipDeleteGraphics(graphics).
#[unsafe(no_mangle)]
pub extern "system" fn GdipDeleteGraphics(graphics: *mut Graphics) -> Status {
    let Some(g) = object(graphics, GRAPHICS_TAG) else { return INVALID_PARAMETER };
    g.tag = 0;
    // SAFETY: a Box this DLL handed out.
    drop(unsafe { Box::from_raw(graphics) });
    OK
}

/// BITMAPINFOHEADER (and a BITMAPINFO without colours, for 32 bpp).
#[repr(C)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_ppm: i32,
    y_ppm: i32,
    clr_used: u32,
    clr_important: u32,
}

/// Composes `w` x `h` premultiplied pixels (`pixel(x, y)`) over the
/// graphics' target at (x, y).
fn compose(
    g: &mut Graphics,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    pixel: impl Fn(i32, i32) -> u32,
) -> Status {
    if w <= 0 || h <= 0 {
        if w < 0 || h < 0 {
            record(b"gdiplus.dll!draw(negative size: mirrored)", 0);
        }
        return OK;
    }
    match g.target {
        Target::Image(p) => {
            let Some(img) = image(p) else { return INVALID_PARAMETER };
            let (x0, y0) = (x.max(0), y.max(0));
            let (x1, y1) = ((x + w).min(img.width as i32), (y + h).min(img.height as i32));
            for ty in y0..y1 {
                for tx in x0..x1 {
                    let s = pixel(tx - x, ty - y);
                    let d = img.get(tx as u32, ty as u32);
                    img.put(tx as u32, ty as u32, over(s, d));
                }
            }
            OK
        }
        Target::Hdc(hdc) => {
            // A top-down 32 bpp DIB section of the pixels, alpha-blended
            // onto the device context with their own (premultiplied)
            // alpha: AC_SRC_OVER, constant alpha 255, AC_SRC_ALPHA.
            let header = BitmapInfoHeader {
                size: 40,
                width: w,
                height: -h,
                planes: 1,
                bit_count: 32,
                compression: 0,
                size_image: 0,
                x_ppm: 0,
                y_ppm: 0,
                clr_used: 0,
                clr_important: 0,
            };
            // SAFETY: gdi32 calls on handles made here; the DIB's bits are
            // w * h u32s while it lives.
            unsafe {
                let mem = CreateCompatibleDC(hdc);
                if mem == 0 {
                    return WIN32_ERROR;
                }
                let mut bits = core::ptr::null_mut();
                let dib = CreateDIBSection(hdc, &header, 0, &mut bits, 0, 0);
                if dib == 0 || bits.is_null() {
                    DeleteDC(mem);
                    return WIN32_ERROR;
                }
                let px = bits.cast::<u32>();
                for yy in 0..h {
                    for xx in 0..w {
                        px.add((yy * w + xx) as usize).write(pixel(xx, yy));
                    }
                }
                let old = SelectObject(mem, dib);
                let done = GdiAlphaBlend(hdc, x, y, w, h, mem, 0, 0, w, h, 0x01ff_0000);
                SelectObject(mem, old);
                DeleteObject(dib);
                DeleteDC(mem);
                if done == 0 { WIN32_ERROR } else { OK }
            }
        }
    }
}

/// GdipFillRectangleI(graphics, brush, x, y, width, height).
#[unsafe(no_mangle)]
pub extern "system" fn GdipFillRectangleI(
    graphics: *mut Graphics,
    brush: *mut Brush,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Status {
    let (Some(g), Some(b)) = (object(graphics, GRAPHICS_TAG), object(brush, BRUSH_TAG)) else {
        return INVALID_PARAMETER;
    };
    let color = premultiply(b.argb);
    compose(g, x, y, width, height, |_, _| color)
}

/// GdipDrawImageRectI(graphics, image, x, y, width, height): the whole
/// image scaled into the rectangle.
#[unsafe(no_mangle)]
pub extern "system" fn GdipDrawImageRectI(
    graphics: *mut Graphics,
    image_: *mut Image,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Status {
    let (Some(g), Some(src)) = (object(graphics, GRAPHICS_TAG), image(image_)) else {
        return INVALID_PARAMETER;
    };
    // Drawing a bitmap onto itself reads what it writes; GDI+ draws from
    // a copy.
    let copy;
    let src: &Image = if matches!(g.target, Target::Image(p) if p == image_) {
        copy = match src.duplicate() {
            Some(c) => c,
            None => return OUT_OF_MEMORY,
        };
        &copy
    } else {
        src
    };
    if width == src.width as i32 && height == src.height as i32 {
        compose(g, x, y, width, height, |sx, sy| src.get(sx as u32, sy as u32))
    } else {
        let (kx, ky) = (src.width as f32 / width as f32, src.height as f32 / height as f32);
        compose(g, x, y, width, height, |sx, sy| {
            src.sample((sx as f32 + 0.5) * kx, (sy as f32 + 0.5) * ky)
        })
    }
}

/// A panic (a bug here): recorded with its place, and the process ends
/// as a failed fast-fail would end it.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut text = [0u8; 80];
    let mut n = 0;
    let line = info.location().map_or(0, |l| {
        for &b in b"gdiplus.dll!panic ".iter().chain(l.file().as_bytes()) {
            if n < text.len() {
                text[n] = b;
                n += 1;
            }
        }
        l.line()
    });
    // SAFETY: a valid byte string.
    unsafe { GalaRecordMissing(text.as_ptr(), n, LEDGER_OTHER, line.into()) };
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
