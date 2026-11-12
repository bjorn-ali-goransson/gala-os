//! IWICBitmap and IWICBitmapLock: pixels in this DLL's memory, as Wine's
//! bitmap tests (run on Windows) and Microsoft's documentation describe
//! them.
//!
//! - Rows are 4-byte aligned (a 3-pixel 24 bpp bitmap has stride 12) and
//!   start zeroed; the resolution starts at 0 x 0 dpi; a palette is
//!   there only once SetPalette gave one (CopyPalette is
//!   WINCODEC_ERR_PALETTEUNAVAILABLE before), and CopyPalette keeps its
//!   type.
//! - Lock(rect, flags): NULL is the whole bitmap, a rectangle that leaves
//!   it E_INVALIDARG. Any number of read locks at once, or one write
//!   lock; anything else is WINCODEC_ERR_ALREADYLOCKED. The lock points
//!   into the bitmap (the rectangle's first pixel), with the bitmap's
//!   stride, its size not counting the last row's padding (3x3 24 bpp:
//!   33 bytes).
//! - CreateBitmapFromMemory copies the caller's rows (a NULL buffer, a
//!   stride of 0 or below a row: E_INVALIDARG; a buffer below `stride *
//!   height`: WINCODEC_ERR_INSUFFICIENTBUFFER).
//! - CreateBitmapFromSource copies a source's pixels, palette (indexed
//!   formats only) and resolution; with WICBitmapNoCache it hands back
//!   the source itself when that is one of these bitmaps, and is
//!   E_NOTIMPL for any other source. CreateBitmapFromSourceRect copies
//!   the part of the rectangle inside the source (a rectangle of no size
//!   or starting outside it: E_INVALIDARG).

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicI32, Ordering};

use crate::format::{self, Fmt, Kind};
use crate::palette;
use crate::{
    E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, Guid, HResult, Header, IID_IWICBITMAP,
    IID_IWICBITMAPLOCK, IID_IWICBITMAPSOURCE, Obj, Rect, S_OK, Source, Spin, VTable,
    WINCODEC_ERR_ALREADYLOCKED, WINCODEC_ERR_INSUFFICIENTBUFFER, WINCODEC_ERR_PALETTEUNAVAILABLE,
    WINCODEC_ERR_UNSUPPORTEDPIXELFORMAT, add_ref, copy_from_memory, put, query, read_guid,
    release_object, resolve_rect, row_bytes, try_box, vtable_of, zeroed,
};

const LOCK_READ: u32 = 1;
const LOCK_WRITE: u32 = 2;
/// CreateBitmapFromSource's WICBitmapNoCache.
const NO_CACHE: u32 = 0;

struct Meta {
    palette: Option<(u32, Vec<u32>)>,
    dpi: (f64, f64),
}

#[repr(C)]
pub struct Bitmap {
    h: Header,
    width: u32,
    height: u32,
    fmt: &'static Fmt,
    stride: usize,
    /// Written through write locks (the buffer itself never moves).
    data: UnsafeCell<Vec<u8>>,
    meta: Spin<Meta>,
    /// Read locks held (> 0), a write lock (-1), or none (0).
    locks: AtomicI32,
}
// SAFETY: `data` is only written by a lock's holder (WIC's contract),
// the rest is atomic or behind a spin lock.
unsafe impl Sync for Bitmap {}

static BITMAP_VTBL: VTable<11> = VTable([
    bitmap_query as *const (),
    add_ref as *const (),
    bitmap_release as *const (),
    get_size as *const (),
    get_pixel_format as *const (),
    get_resolution as *const (),
    copy_palette as *const (),
    copy_pixels as *const (),
    lock as *const (),
    set_palette as *const (),
    set_resolution as *const (),
]);

fn this<'a>(o: Obj) -> &'a Bitmap {
    // SAFETY: one of our bitmaps.
    unsafe { &*(o as *const Bitmap) }
}

impl Bitmap {
    fn bytes(&self) -> &[u8] {
        // SAFETY: the buffer lives as long as the bitmap; writers go
        // through locks.
        unsafe { &*self.data.get() }
    }
}

/// A new zeroed bitmap (one reference).
fn new_bitmap(width: u32, height: u32, fmt: &'static Fmt) -> Result<*mut Bitmap, HResult> {
    let stride = row_bytes(width, fmt.bpp).next_multiple_of(4);
    let size = stride.checked_mul(height as usize).ok_or(E_OUTOFMEMORY)?;
    let data = zeroed(size).ok_or(E_OUTOFMEMORY)?;
    try_box(Bitmap {
        h: Header::new(BITMAP_VTBL.addr()),
        width,
        height,
        fmt,
        stride,
        data: UnsafeCell::new(data),
        meta: Spin::new(Meta { palette: None, dpi: (0.0, 0.0) }),
        locks: AtomicI32::new(0),
    })
    .ok_or(E_OUTOFMEMORY)
}

/// IWICImagingFactory::CreateBitmap.
pub fn create(width: u32, height: u32, format: *const Guid, out: *mut Obj) -> HResult {
    if out.is_null() || width == 0 || height == 0 {
        return E_INVALIDARG;
    }
    let Some(g) = read_guid(format) else { return E_INVALIDARG };
    let Some(fmt) = format::find(&g) else {
        crate::gap(b"CreateBitmap(pixel format)", crate::data1(&g));
        return WINCODEC_ERR_UNSUPPORTEDPIXELFORMAT;
    };
    match new_bitmap(width, height, fmt) {
        Ok(b) => {
            put(out, b as Obj);
            S_OK
        }
        Err(e) => e,
    }
}

/// IWICImagingFactory::CreateBitmapFromMemory.
#[allow(clippy::too_many_arguments)]
pub fn create_from_memory(
    width: u32,
    height: u32,
    format: *const Guid,
    stride: u32,
    size: u32,
    buffer: *const u8,
    out: *mut Obj,
) -> HResult {
    if out.is_null() || buffer.is_null() || width == 0 || height == 0 {
        return E_INVALIDARG;
    }
    let Some(g) = read_guid(format) else { return E_INVALIDARG };
    let Some(fmt) = format::find(&g) else {
        crate::gap(b"CreateBitmapFromMemory(pixel format)", crate::data1(&g));
        return WINCODEC_ERR_UNSUPPORTEDPIXELFORMAT;
    };
    let row = row_bytes(width, fmt.bpp);
    if (stride as usize) < row {
        return E_INVALIDARG;
    }
    if (size as u64) < u64::from(stride) * u64::from(height) {
        return WINCODEC_ERR_INSUFFICIENTBUFFER;
    }
    let b = match new_bitmap(width, height, fmt) {
        Ok(b) => b,
        Err(e) => return e,
    };
    // SAFETY: our new bitmap, not shared yet; the caller's buffer of
    // `size` bytes.
    unsafe {
        let data = &mut *(*b).data.get();
        let src = core::slice::from_raw_parts(buffer, size as usize);
        for y in 0..height as usize {
            let s = &src[y * stride as usize..y * stride as usize + row];
            data[y * (*b).stride..y * (*b).stride + row].copy_from_slice(s);
        }
    }
    put(out, b as Obj);
    S_OK
}

/// IWICImagingFactory::CreateBitmapFromSource and ...FromSourceRect
/// (`rect` None: all of the source).
pub fn create_from_source(
    source: Obj,
    option: u32,
    rect: Option<(u32, u32, u32, u32)>,
    out: *mut Obj,
) -> HResult {
    if source.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    if rect.is_none() && option == NO_CACHE {
        if vtable_of(source) == BITMAP_VTBL.addr() {
            add_ref(source);
            put(out, source);
            return S_OK;
        }
        return E_NOTIMPL;
    }
    let src = Source(source);
    let (sw, sh) = match src.size() {
        Ok(s) => s,
        Err(e) => return e,
    };
    let (x, y, w, h) = match rect {
        None => (0, 0, sw, sh),
        Some((x, y, w, h)) => {
            if w == 0 || h == 0 || x >= sw || y >= sh {
                return E_INVALIDARG;
            }
            (x, y, w.min(sw - x), h.min(sh - y))
        }
    };
    let g = match src.format() {
        Ok(g) => g,
        Err(e) => return e,
    };
    let Some(fmt) = format::find(&g) else {
        crate::gap(b"CreateBitmapFromSource(pixel format)", crate::data1(&g));
        return WINCODEC_ERR_UNSUPPORTEDPIXELFORMAT;
    };
    let b = match new_bitmap(w, h, fmt) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let r = Rect { x: x as i32, y: y as i32, w: w as i32, h: h as i32 };
    // SAFETY: our new bitmap, not shared yet.
    let (stride, data) = unsafe { ((*b).stride, &mut *(*b).data.get()) };
    let hr = src.copy_pixels(&r, stride as u32, data.len() as u32, data.as_mut_ptr());
    if hr != S_OK {
        release_object::<Bitmap>(b as Obj);
        return hr;
    }
    let palette = if fmt.kind == Kind::Indexed {
        let p = palette::new_palette();
        p.and_then(|p| {
            let r = if src.copy_palette(p) == S_OK { palette::colors(p) } else { None };
            release_object::<palette::Palette>(p);
            r
        })
    } else {
        None
    };
    let dpi = src.resolution().unwrap_or((0.0, 0.0));
    // SAFETY: as above.
    unsafe { (*b).meta.with(|m| *m = Meta { palette, dpi }) };
    put(out, b as Obj);
    S_OK
}

extern "system" fn bitmap_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    query(this, &[IID_IWICBITMAP, IID_IWICBITMAPSOURCE], riid, out)
}

extern "system" fn bitmap_release(this: Obj) -> u32 {
    release_object::<Bitmap>(this)
}

extern "system" fn get_size(this_: Obj, w: *mut u32, h: *mut u32) -> HResult {
    if w.is_null() || h.is_null() {
        return E_INVALIDARG;
    }
    let b = this(this_);
    put(w, b.width);
    put(h, b.height);
    S_OK
}

extern "system" fn get_pixel_format(this_: Obj, out: *mut Guid) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, this(this_).fmt.guid);
    S_OK
}

extern "system" fn get_resolution(this_: Obj, x: *mut f64, y: *mut f64) -> HResult {
    if x.is_null() || y.is_null() {
        return E_INVALIDARG;
    }
    let (dx, dy) = this(this_).meta.with(|m| m.dpi);
    put(x, dx);
    put(y, dy);
    S_OK
}

extern "system" fn set_resolution(this_: Obj, x: f64, y: f64) -> HResult {
    this(this_).meta.with(|m| m.dpi = (x, y));
    S_OK
}

extern "system" fn copy_palette(this_: Obj, palette: Obj) -> HResult {
    if palette.is_null() {
        return E_INVALIDARG;
    }
    match this(this_).meta.with(|m| m.palette.clone()) {
        None => WINCODEC_ERR_PALETTEUNAVAILABLE,
        Some((kind, colors)) => palette::assign(palette, kind, &colors),
    }
}

extern "system" fn set_palette(this_: Obj, palette: Obj) -> HResult {
    if palette.is_null() {
        return E_INVALIDARG;
    }
    match palette::colors(palette) {
        Some(p) => {
            this(this_).meta.with(|m| m.palette = Some(p));
            S_OK
        }
        None => E_OUTOFMEMORY,
    }
}

extern "system" fn copy_pixels(
    this_: Obj,
    rect: *const Rect,
    stride: u32,
    size: u32,
    buf: *mut u8,
) -> HResult {
    let b = this(this_);
    copy_from_memory(b.bytes(), b.stride, b.fmt.bpp, b.width, b.height, rect, stride, size, buf)
}

/// A lock on part of a bitmap (holds a reference on it).
#[repr(C)]
struct Lock {
    h: Header,
    bitmap: Obj,
    rect: (u32, u32, u32, u32),
    write: bool,
}

impl Drop for Lock {
    fn drop(&mut self) {
        let b = this(self.bitmap);
        if self.write {
            b.locks.store(0, Ordering::Release);
        } else {
            b.locks.fetch_sub(1, Ordering::AcqRel);
        }
        release_object::<Bitmap>(self.bitmap);
    }
}

static LOCK_VTBL: VTable<7> = VTable([
    lock_query as *const (),
    add_ref as *const (),
    lock_release as *const (),
    lock_get_size as *const (),
    lock_get_stride as *const (),
    lock_get_data_pointer as *const (),
    lock_get_pixel_format as *const (),
]);

extern "system" fn lock(this_: Obj, rect: *const Rect, flags: u32, out: *mut Obj) -> HResult {
    if out.is_null() || flags & (LOCK_READ | LOCK_WRITE) == 0 {
        return E_INVALIDARG;
    }
    let b = this(this_);
    let r = match resolve_rect(rect, b.width, b.height) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let write = flags & LOCK_WRITE != 0;
    let taken = if write {
        b.locks.compare_exchange(0, -1, Ordering::AcqRel, Ordering::Relaxed).is_ok()
    } else {
        let mut n = b.locks.load(Ordering::Relaxed);
        loop {
            if n < 0 {
                break false;
            }
            match b.locks.compare_exchange_weak(n, n + 1, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => break true,
                Err(now) => n = now,
            }
        }
    };
    if !taken {
        return WINCODEC_ERR_ALREADYLOCKED;
    }
    add_ref(this_);
    match try_box(Lock { h: Header::new(LOCK_VTBL.addr()), bitmap: this_, rect: r, write }) {
        Some(l) => {
            put(out, l as Obj);
            S_OK
        }
        // The lock try_box could not place was dropped: that undid what
        // was taken.
        None => E_OUTOFMEMORY,
    }
}

fn the_lock<'a>(o: Obj) -> &'a Lock {
    // SAFETY: one of our locks.
    unsafe { &*(o as *const Lock) }
}

extern "system" fn lock_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    query(this, &[IID_IWICBITMAPLOCK], riid, out)
}

extern "system" fn lock_release(this: Obj) -> u32 {
    release_object::<Lock>(this)
}

extern "system" fn lock_get_size(this_: Obj, w: *mut u32, h: *mut u32) -> HResult {
    if w.is_null() || h.is_null() {
        return E_INVALIDARG;
    }
    let l = the_lock(this_);
    put(w, l.rect.2);
    put(h, l.rect.3);
    S_OK
}

extern "system" fn lock_get_stride(this_: Obj, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, this(the_lock(this_).bitmap).stride as u32);
    S_OK
}

extern "system" fn lock_get_data_pointer(
    this_: Obj,
    size: *mut u32,
    data: *mut *mut u8,
) -> HResult {
    if size.is_null() || data.is_null() {
        return E_INVALIDARG;
    }
    let l = the_lock(this_);
    let b = this(l.bitmap);
    let (x, y, w, h) = l.rect;
    let start = y as usize * b.stride + x as usize * b.fmt.bpp as usize / 8;
    let len = b.stride * (h as usize - 1) + row_bytes(w, b.fmt.bpp);
    // SAFETY: the bitmap's buffer, which outlives the lock; the
    // rectangle is inside it.
    let p = unsafe { (*b.data.get()).as_mut_ptr().add(start) };
    put(size, len as u32);
    put(data, p);
    S_OK
}

extern "system" fn lock_get_pixel_format(this_: Obj, out: *mut Guid) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, this(the_lock(this_).bitmap).fmt.guid);
    S_OK
}
