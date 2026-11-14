//! The sources that wrap another source: the format converter
//! (IWICFormatConverter), the scaler (IWICBitmapScaler) and the clipper
//! (IWICBitmapClipper), as Wine's converter and bitmap tests (run on
//! Windows) describe them. Each holds a reference on its source and asks
//! it for pixels on every CopyPixels (nothing is cached).
//!
//! - Before Initialize: GetSize, GetResolution and CopyPixels are
//!   WINCODEC_ERR_NOTINITIALIZED (before any argument check), CopyPalette
//!   WINCODEC_ERR_PALETTEUNAVAILABLE, GetPixelFormat
//!   GUID_WICPixelFormatDontCare; a second Initialize is
//!   WINCODEC_ERR_WRONGSTATE. After it, NULL outputs are E_INVALIDARG and
//!   size, resolution and palette are the source's (the scaler's size its
//!   own, the clipper's its rectangle's).
//! - The converter (`format`): the source's pixels in the destination
//!   format, the caller's rectangle asked of the source; the same format
//!   is passed straight through. A pair it cannot convert is
//!   WINCODEC_ERR_UNSUPPORTEDOPERATION at Initialize (recorded, the two
//!   formats' first dwords as detail) and FALSE from CanConvert. Dither,
//!   palette and alpha threshold only matter to indexed destinations,
//!   which are not made.
//! - The scaler: Initialize with no source or a width or height of 0 is
//!   E_INVALIDARG. Its format is the source's. NearestNeighbor takes the
//!   source pixel under each destination pixel's centre; Linear (and,
//!   for now, Cubic, Fant and HighQualityCubic: recorded as such)
//!   interpolates bilinearly between the four source pixels around the
//!   centre, channel by channel, in formats with a byte per channel;
//!   other formats take the nearest pixel (recorded).
//! - The clipper: Initialize with a rectangle that leaves the source is
//!   E_INVALIDARG; CopyPixels' rectangle is relative to the clipper's and
//!   must stay inside it (E_INVALIDARG): the source is asked for that
//!   rectangle moved by the clipper's origin.

use alloc::vec::Vec;

use crate::format::{self, Fmt, Kind};
use crate::{
    E_INVALIDARG, E_OUTOFMEMORY, Guid, HResult, Header, IID_IWICBITMAPSCALER, IID_IWICBITMAPSOURCE,
    IID_IWICFORMATCONVERTER, Obj, Rect, S_OK, Source, Spin, VTable, WINCODEC_ERR_NOTINITIALIZED,
    WINCODEC_ERR_PALETTEUNAVAILABLE, WINCODEC_ERR_UNSUPPORTEDOPERATION, WINCODEC_ERR_WRONGSTATE,
    add_ref, data1, gap, guid, out_buffer, put, query, read_guid, release_object, resolve_rect,
    row_bytes, try_box, zeroed,
};

const IID_IWICBITMAPCLIPPER: Guid =
    guid(0xe4fb_cf03, 0x223d, 0x4e81, [0x93, 0x33, 0xd6, 0x35, 0x55, 0x6d, 0xd1, 0xb5]);

const NEAREST_NEIGHBOR: u32 = 0;
const LINEAR: u32 = 1;

/// What a wrapper was initialized with.
#[derive(Clone, Copy)]
enum Setup {
    None,
    /// The converter: the source, its format and the destination's.
    Convert {
        src: Source,
        from: &'static Fmt,
        to: &'static Fmt,
    },
    /// The scaler: the source, its format, its size, ours and the mode.
    Scale {
        src: Source,
        fmt: &'static Fmt,
        sw: u32,
        sh: u32,
        w: u32,
        h: u32,
        mode: u32,
    },
    /// The clipper: the source and the rectangle.
    Clip {
        src: Source,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    },
}

// SAFETY: the source is a COM object the caller made free-threaded or
// keeps to one thread, as WIC's own are used.
unsafe impl Send for Setup {}

impl Setup {
    fn source(self) -> Option<Source> {
        match self {
            Setup::None => None,
            Setup::Convert { src, .. } | Setup::Scale { src, .. } | Setup::Clip { src, .. } => {
                Some(src)
            }
        }
    }
}

#[repr(C)]
pub struct Wrapper {
    h: Header,
    setup: Spin<Setup>,
}

impl Drop for Wrapper {
    fn drop(&mut self) {
        if let Some(s) = self.setup.with(|s| s.source()) {
            s.release();
        }
    }
}

static CONVERTER_VTBL: VTable<10> = VTable([
    converter_query as *const (),
    add_ref as *const (),
    wrapper_release as *const (),
    get_size as *const (),
    get_pixel_format as *const (),
    get_resolution as *const (),
    copy_palette as *const (),
    copy_pixels as *const (),
    converter_initialize as *const (),
    can_convert as *const (),
]);

static SCALER_VTBL: VTable<9> = VTable([
    scaler_query as *const (),
    add_ref as *const (),
    wrapper_release as *const (),
    get_size as *const (),
    get_pixel_format as *const (),
    get_resolution as *const (),
    copy_palette as *const (),
    copy_pixels as *const (),
    scaler_initialize as *const (),
]);

static CLIPPER_VTBL: VTable<9> = VTable([
    clipper_query as *const (),
    add_ref as *const (),
    wrapper_release as *const (),
    get_size as *const (),
    get_pixel_format as *const (),
    get_resolution as *const (),
    copy_palette as *const (),
    copy_pixels as *const (),
    clipper_initialize as *const (),
]);

pub enum Kinds {
    Converter,
    Scaler,
    Clipper,
}

/// A new, uninitialized wrapper (the factory's CreateFormatConverter,
/// CreateBitmapScaler, CreateBitmapClipper).
pub fn create(kind: Kinds, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let vtbl = match kind {
        Kinds::Converter => CONVERTER_VTBL.addr(),
        Kinds::Scaler => SCALER_VTBL.addr(),
        Kinds::Clipper => CLIPPER_VTBL.addr(),
    };
    match try_box(Wrapper { h: Header::new(vtbl), setup: Spin::new(Setup::None) }) {
        Some(w) => {
            put(out, w as Obj);
            S_OK
        }
        None => E_OUTOFMEMORY,
    }
}

fn setup(o: Obj) -> Setup {
    // SAFETY: one of our wrappers.
    unsafe { (*(o as *const Wrapper)).setup.with(|s| *s) }
}

/// Stores `s` as the wrapper's setup if it has none (taking a reference
/// on the source); WINCODEC_ERR_WRONGSTATE otherwise.
fn set_up(o: Obj, s: Setup) -> HResult {
    if let Some(src) = s.source() {
        src.add_ref();
    }
    // SAFETY: one of our wrappers.
    let taken = unsafe {
        (*(o as *const Wrapper)).setup.with(|cur| {
            if matches!(cur, Setup::None) {
                *cur = s;
                true
            } else {
                false
            }
        })
    };
    if taken {
        S_OK
    } else {
        if let Some(src) = s.source() {
            src.release();
        }
        WINCODEC_ERR_WRONGSTATE
    }
}

extern "system" fn converter_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    query(this, &[IID_IWICFORMATCONVERTER, IID_IWICBITMAPSOURCE], riid, out)
}

extern "system" fn scaler_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    query(this, &[IID_IWICBITMAPSCALER, IID_IWICBITMAPSOURCE], riid, out)
}

extern "system" fn clipper_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    query(this, &[IID_IWICBITMAPCLIPPER, IID_IWICBITMAPSOURCE], riid, out)
}

extern "system" fn wrapper_release(this: Obj) -> u32 {
    release_object::<Wrapper>(this)
}

extern "system" fn get_size(this: Obj, w: *mut u32, h: *mut u32) -> HResult {
    let (ow, oh) = match setup(this) {
        Setup::None => return WINCODEC_ERR_NOTINITIALIZED,
        Setup::Convert { src, .. } => match src.size() {
            Ok(s) => s,
            Err(e) => return e,
        },
        Setup::Scale { w, h, .. } | Setup::Clip { w, h, .. } => (w, h),
    };
    if w.is_null() || h.is_null() {
        return E_INVALIDARG;
    }
    put(w, ow);
    put(h, oh);
    S_OK
}

extern "system" fn get_pixel_format(this: Obj, out: *mut Guid) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let g = match setup(this) {
        Setup::None => format::DONT_CARE,
        Setup::Convert { to, .. } => to.guid,
        Setup::Scale { fmt, .. } => fmt.guid,
        Setup::Clip { src, .. } => match src.format() {
            Ok(g) => g,
            Err(e) => return e,
        },
    };
    put(out, g);
    S_OK
}

extern "system" fn get_resolution(this: Obj, x: *mut f64, y: *mut f64) -> HResult {
    let Some(src) = setup(this).source() else { return WINCODEC_ERR_NOTINITIALIZED };
    if x.is_null() || y.is_null() {
        return E_INVALIDARG;
    }
    match src.resolution() {
        Ok((dx, dy)) => {
            put(x, dx);
            put(y, dy);
            S_OK
        }
        Err(e) => e,
    }
}

extern "system" fn copy_palette(this: Obj, palette: Obj) -> HResult {
    if palette.is_null() {
        return E_INVALIDARG;
    }
    match setup(this) {
        Setup::None => WINCODEC_ERR_PALETTEUNAVAILABLE,
        // A converted image has no palette (indexed formats are not
        // made).
        Setup::Convert { from, to, src } => {
            if from.guid == to.guid {
                src.copy_palette(palette)
            } else {
                WINCODEC_ERR_PALETTEUNAVAILABLE
            }
        }
        s => s.source().map_or(WINCODEC_ERR_PALETTEUNAVAILABLE, |src| src.copy_palette(palette)),
    }
}

extern "system" fn copy_pixels(
    this: Obj,
    rect: *const Rect,
    stride: u32,
    size: u32,
    buf: *mut u8,
) -> HResult {
    match setup(this) {
        Setup::None => WINCODEC_ERR_NOTINITIALIZED,
        Setup::Convert { src, from, to } => convert(src, from, to, rect, stride, size, buf),
        Setup::Scale { src, fmt, sw, sh, w, h, mode } => {
            scale(src, fmt, (sw, sh), (w, h), mode, rect, stride, size, buf)
        }
        Setup::Clip { src, x, y, w, h } => {
            let (rx, ry, rw, rh) = match resolve_rect(rect, w, h) {
                Ok(r) => r,
                Err(e) => return e,
            };
            let r = Rect { x: (x + rx) as i32, y: (y + ry) as i32, w: rw as i32, h: rh as i32 };
            src.copy_pixels(&r, stride, size, buf)
        }
    }
}

/// Whether the converter turns `from` into `to`.
fn convertible(from: &Fmt, to: &Fmt) -> bool {
    from.guid == to.guid || format::writable(to)
}

extern "system" fn can_convert(
    _this: Obj,
    from: *const Guid,
    to: *const Guid,
    out: *mut i32,
) -> HResult {
    let (Some(f), Some(t)) = (read_guid(from), read_guid(to)) else { return E_INVALIDARG };
    if out.is_null() {
        return E_INVALIDARG;
    }
    let ok =
        matches!((format::find(&f), format::find(&t)), (Some(a), Some(b)) if convertible(a, b));
    put(out, i32::from(ok));
    S_OK
}

/// The converter's Initialize: the pair checked (and recorded when it
/// cannot be converted).
pub fn converter_init(this: Obj, source: Obj, to: *const Guid) -> HResult {
    let Some(t) = read_guid(to) else { return E_INVALIDARG };
    if source.is_null() {
        return E_INVALIDARG;
    }
    let src = Source(source);
    let f = match src.format() {
        Ok(f) => f,
        Err(e) => return e,
    };
    match (format::find(&f), format::find(&t)) {
        (Some(from), Some(to)) if convertible(from, to) => {
            set_up(this, Setup::Convert { src, from, to })
        }
        _ => {
            gap(b"IWICFormatConverter::Initialize(formats)", data1(&f) << 32 | data1(&t));
            WINCODEC_ERR_UNSUPPORTEDOPERATION
        }
    }
}

extern "system" fn converter_initialize(
    this: Obj,
    source: Obj,
    to: *const Guid,
    _dither: u32,
    _palette: Obj,
    _alpha_threshold: f64,
    _palette_type: u32,
) -> HResult {
    converter_init(this, source, to)
}

/// The source's pixels in rectangle `r` (x, y, w, h), in its own
/// format: rows `row_bytes(w, bpp)` apart.
fn fetch(src: Source, fmt: &Fmt, r: (u32, u32, u32, u32)) -> Result<Vec<u8>, HResult> {
    let row = row_bytes(r.2, fmt.bpp);
    let size = row.checked_mul(r.3 as usize).ok_or(E_OUTOFMEMORY)?;
    let mut tmp = zeroed(size).ok_or(E_OUTOFMEMORY)?;
    let rect = Rect { x: r.0 as i32, y: r.1 as i32, w: r.2 as i32, h: r.3 as i32 };
    match src.copy_pixels(&rect, row as u32, size as u32, tmp.as_mut_ptr()) {
        S_OK => Ok(tmp),
        e => Err(e),
    }
}

#[allow(clippy::too_many_arguments)]
fn convert(
    src: Source,
    from: &'static Fmt,
    to: &'static Fmt,
    rect: *const Rect,
    stride: u32,
    size: u32,
    buf: *mut u8,
) -> HResult {
    if from.guid == to.guid {
        return src.copy_pixels(rect, stride, size, buf);
    }
    let (sw, sh) = match src.size() {
        Ok(s) => s,
        Err(e) => return e,
    };
    let r = match resolve_rect(rect, sw, sh) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let out = match out_buffer(row_bytes(r.2, to.bpp), r.3, stride, size, buf) {
        Ok(o) => o,
        Err(e) => return e,
    };
    let tmp = match fetch(src, from, r) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let palette = if from.kind == Kind::Indexed { src.palette_colors() } else { None };
    let palette = palette.as_deref().unwrap_or(&[]);
    let srow = row_bytes(r.2, from.bpp);
    for y in 0..r.3 as usize {
        let line = &tmp[y * srow..(y + 1) * srow];
        let dst = &mut out[y * stride as usize..];
        for x in 0..r.2 as usize {
            format::write(to, dst, x, format::read(from, line, x, palette));
        }
    }
    S_OK
}

extern "system" fn scaler_initialize(this: Obj, source: Obj, w: u32, h: u32, mode: u32) -> HResult {
    if source.is_null() || w == 0 || h == 0 {
        return E_INVALIDARG;
    }
    let src = Source(source);
    let (sw, sh) = match src.size() {
        Ok(s) => s,
        Err(e) => return e,
    };
    let g = match src.format() {
        Ok(g) => g,
        Err(e) => return e,
    };
    let Some(fmt) = format::find(&g) else {
        gap(b"IWICBitmapScaler::Initialize(pixel format)", data1(&g));
        return WINCODEC_ERR_UNSUPPORTEDOPERATION;
    };
    if mode > 4 {
        return E_INVALIDARG;
    }
    if mode > LINEAR {
        gap(b"IWICBitmapScaler(mode done as Linear)", mode.into());
    }
    if mode != NEAREST_NEIGHBOR && !format::byte_channels(fmt) {
        gap(b"IWICBitmapScaler(interpolation of a packed format)", data1(&g));
    }
    set_up(this, Setup::Scale { src, fmt, sw, sh, w, h, mode })
}

/// The source coordinate under destination pixel `i`'s centre, in 16.16
/// fixed point, minus half a pixel (the left/upper neighbour's centre is
/// at its floor), clamped to the source.
fn centre(i: u32, from: u32, to: u32) -> i64 {
    let v = (((2 * i64::from(i) + 1) * i64::from(from)) << 16) / (2 * i64::from(to)) - (1 << 15);
    v.clamp(0, (i64::from(from) - 1) << 16)
}

#[allow(clippy::too_many_arguments)]
fn scale(
    src: Source,
    fmt: &'static Fmt,
    (sw, sh): (u32, u32),
    (w, h): (u32, u32),
    mode: u32,
    rect: *const Rect,
    stride: u32,
    size: u32,
    buf: *mut u8,
) -> HResult {
    let (rx, ry, rw, rh) = match resolve_rect(rect, w, h) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let row = row_bytes(rw, fmt.bpp);
    let out = match out_buffer(row, rh, stride, size, buf) {
        Ok(o) => o,
        Err(e) => return e,
    };
    // The source as it is now (the whole of it: a destination row may
    // reach any source row).
    let whole = match fetch(src, fmt, (0, 0, sw, sh)) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let srow = row_bytes(sw, fmt.bpp);
    let linear = mode != NEAREST_NEIGHBOR && format::byte_channels(fmt);
    let bytes = (fmt.bpp / 8) as usize;
    for j in 0..rh as usize {
        let dy = ry + j as u32;
        let dst = &mut out[j * stride as usize..j * stride as usize + row];
        if !linear {
            let sy = ((2 * u64::from(dy) + 1) * u64::from(sh) / (2 * u64::from(h))) as usize;
            let line = &whole[sy * srow..];
            for i in 0..rw as usize {
                let dx = rx + i as u32;
                let sx = ((2 * u64::from(dx) + 1) * u64::from(sw) / (2 * u64::from(w))) as usize;
                if fmt.bpp.is_multiple_of(8) {
                    dst[i * bytes..(i + 1) * bytes]
                        .copy_from_slice(&line[sx * bytes..(sx + 1) * bytes]);
                } else {
                    crate::put_bits(dst, i, fmt.bpp, crate::get_bits(line, sx, fmt.bpp));
                }
            }
            continue;
        }
        let cy = centre(dy, sh, h);
        let (y0, fy) = ((cy >> 16) as usize, (cy & 0xffff) as u64);
        let y1 = (y0 + 1).min(sh as usize - 1);
        for i in 0..rw as usize {
            let cx = centre(rx + i as u32, sw, w);
            let (x0, fx) = ((cx >> 16) as usize, (cx & 0xffff) as u64);
            let x1 = (x0 + 1).min(sw as usize - 1);
            for k in 0..bytes {
                let p = |x: usize, y: usize| u64::from(whole[y * srow + x * bytes + k]);
                let top = p(x0, y0) * (0x10000 - fx) + p(x1, y0) * fx;
                let bottom = p(x0, y1) * (0x10000 - fx) + p(x1, y1) * fx;
                let v = (top * (0x10000 - fy) + bottom * fy + (1 << 31)) >> 32;
                dst[i * bytes + k] = v.min(255) as u8;
            }
        }
    }
    S_OK
}

extern "system" fn clipper_initialize(this: Obj, source: Obj, rect: *const Rect) -> HResult {
    if source.is_null() || rect.is_null() {
        return E_INVALIDARG;
    }
    let src = Source(source);
    let (sw, sh) = match src.size() {
        Ok(s) => s,
        Err(e) => return e,
    };
    match resolve_rect(rect, sw, sh) {
        Ok((x, y, w, h)) => set_up(this, Setup::Clip { src, x, y, w, h }),
        Err(e) => e,
    }
}
