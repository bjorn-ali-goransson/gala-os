//! Brushes (solid colour, linear and radial gradients, bitmap), gradient
//! stop collections and bitmaps. A render target makes them; a draw
//! turns the brush into a `raster::Paint` through the brush's transform,
//! the target's transform and its DPI scale.

use alloc::vec::Vec;

use crate::raster::{self, EXTEND_CLAMP, Paint, Pixels, Ramp, Rgba};
use crate::{
    ALPHA_IGNORE, ALPHA_PREMULTIPLIED, ColorF, ComRef, D2DERR_UNSUPPORTED_PIXEL_FORMAT,
    DXGI_FORMAT_B8G8R8A8_UNORM, E_INVALIDARG, E_OUTOFMEMORY, Guid, HResult, IID_BITMAP,
    IID_BITMAP_BRUSH, IID_BRUSH, IID_GRADIENT_STOPS, IID_IMAGE, IID_LINEAR_BRUSH, IID_RADIAL_BRUSH,
    IID_RESOURCE, IID_SOLID_BRUSH, Matrix, Obj, PixelFormat, Point, RectF, Res, S_OK, SizeF, SizeU,
    Spin, VTable, add_ref, get_factory, give, not_implemented, put, query, read, release_object,
    vtable_of,
};

// ---- Gradient stop collections ----------------------------------------------

/// D2D1_GRADIENT_STOP.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Stop {
    pub position: f32,
    pub color: ColorF,
}

#[repr(C)]
pub struct Stops {
    res: Res,
    given: Vec<Stop>,
    gamma: u32,
    ramp: Ramp,
}

static STOPS_VTBL: VTable<8> = VTable([
    stops_query as *const (),
    add_ref as *const (),
    stops_release as *const (),
    get_factory as *const (),
    stops_GetGradientStopCount as *const (),
    stops_GetGradientStops as *const (),
    stops_GetColorInterpolationGamma as *const (),
    stops_GetExtendMode as *const (),
]);

/// CreateGradientStopCollection: the stops as given (sorted by position
/// for drawing, premultiplied); none is E_INVALIDARG. D2D1_GAMMA_1_0
/// (interpolation in linear light) is drawn as 2.2 and recorded.
pub fn create_stops(
    factory: Obj,
    stops: *const Stop,
    n: u32,
    gamma: u32,
    extend: u32,
    out: *mut Obj,
) -> HResult {
    if out.is_null() {
        return crate::E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if stops.is_null() || n == 0 {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's `n` stops.
    let given_slice = unsafe { core::slice::from_raw_parts(stops, n as usize) };
    let mut given = Vec::new();
    let mut sorted: Vec<(f32, Rgba)> = Vec::new();
    if given.try_reserve_exact(given_slice.len()).is_err()
        || sorted.try_reserve_exact(given_slice.len()).is_err()
    {
        return E_OUTOFMEMORY;
    }
    given.extend_from_slice(given_slice);
    for s in given_slice {
        sorted.push((s.position, Rgba::premultiplied(s.color, 1.0)));
    }
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    if gamma == 1 {
        crate::gap(b"CreateGradientStopCollection(D2D1_GAMMA_1_0)", 0);
    }
    if extend > raster::EXTEND_MIRROR {
        return E_INVALIDARG;
    }
    give(
        out,
        Stops {
            res: Res::new(STOPS_VTBL.addr(), factory),
            given,
            gamma,
            ramp: Ramp { stops: sorted, extend },
        },
    )
}

fn stops<'a>(o: Obj) -> &'a Stops {
    // SAFETY: one of our stop collections.
    unsafe { &*(o as *const Stops) }
}

pub fn is_stops(o: Obj) -> bool {
    vtable_of(o) == STOPS_VTBL.addr()
}

extern "system" fn stops_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    query(this, b"ID2D1GradientStopCollection", &[IID_RESOURCE, IID_GRADIENT_STOPS], riid, out)
}

extern "system" fn stops_release(this: Obj) -> u32 {
    release_object::<Stops>(this)
}

extern "system" fn stops_GetGradientStopCount(this: Obj) -> u32 {
    stops(this).given.len() as u32
}

extern "system" fn stops_GetGradientStops(this: Obj, out: *mut Stop, n: u32) {
    if out.is_null() {
        return;
    }
    for (i, s) in stops(this).given.iter().take(n as usize).enumerate() {
        // SAFETY: the caller's array of `n` stops.
        unsafe { out.add(i).write_unaligned(*s) };
    }
}

extern "system" fn stops_GetColorInterpolationGamma(this: Obj) -> u32 {
    stops(this).gamma
}

extern "system" fn stops_GetExtendMode(this: Obj) -> u32 {
    stops(this).ramp.extend
}

// ---- Bitmaps ------------------------------------------------------------------

#[repr(C)]
pub struct Bitmap {
    res: Res,
    pub w: u32,
    pub h: u32,
    pub dpi: (f32, f32),
    pub format: PixelFormat,
    /// Premultiplied BGRA, rows `w * 4` bytes apart.
    pub pixels: Spin<Vec<u8>>,
}

static BITMAP_VTBL: VTable<11> = VTable([
    bitmap_query as *const (),
    add_ref as *const (),
    bitmap_release as *const (),
    get_factory as *const (),
    bitmap_GetSize as *const (),
    bitmap_GetPixelSize as *const (),
    bitmap_GetPixelFormat as *const (),
    bitmap_GetDpi as *const (),
    bitmap_CopyFromBitmap as *const (),
    bitmap_CopyFromRenderTarget as *const (),
    bitmap_CopyFromMemory as *const (),
]);

/// D2D1_BITMAP_PROPERTIES.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BitmapProps {
    pub format: PixelFormat,
    pub dpi_x: f32,
    pub dpi_y: f32,
}

pub fn is_bitmap(o: Obj) -> bool {
    vtable_of(o) == BITMAP_VTBL.addr()
}

pub fn bitmap<'a>(o: Obj) -> &'a Bitmap {
    // SAFETY: one of our bitmaps.
    unsafe { &*(o as *const Bitmap) }
}

/// The pixel format a new bitmap gets: B8G8R8A8 premultiplied or
/// ignoring alpha (UNKNOWN parts from `fallback`, the target's); others
/// D2DERR_UNSUPPORTED_PIXEL_FORMAT.
fn bitmap_format(want: PixelFormat, fallback: PixelFormat) -> Result<PixelFormat, HResult> {
    let format = if want.format == 0 { fallback.format } else { want.format };
    let alpha = if want.alpha == 0 { fallback.alpha } else { want.alpha };
    if format != DXGI_FORMAT_B8G8R8A8_UNORM
        || (alpha != ALPHA_PREMULTIPLIED && alpha != ALPHA_IGNORE)
    {
        return Err(D2DERR_UNSUPPORTED_PIXEL_FORMAT);
    }
    Ok(PixelFormat { format, alpha })
}

fn dpi_of(props: Option<BitmapProps>) -> (f32, f32) {
    match props {
        Some(p) if p.dpi_x > 0.0 && p.dpi_y > 0.0 => (p.dpi_x, p.dpi_y),
        _ => (96.0, 96.0),
    }
}

fn new_bitmap(
    factory: Obj,
    w: u32,
    h: u32,
    dpi: (f32, f32),
    format: PixelFormat,
    pixels: Vec<u8>,
    out: *mut Obj,
) -> HResult {
    give(
        out,
        Bitmap {
            res: Res::new(BITMAP_VTBL.addr(), factory),
            w,
            h,
            dpi,
            format,
            pixels: Spin::new(pixels),
        },
    )
}

/// CreateBitmap(size, source, pitch, properties): the caller's rows (or
/// zeroes for NULL).
pub fn create_bitmap(
    factory: Obj,
    target_format: PixelFormat,
    size: SizeU,
    src: *const u8,
    pitch: u32,
    props: *const BitmapProps,
    out: *mut Obj,
) -> HResult {
    if out.is_null() {
        return crate::E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let p = read(props);
    let format = match bitmap_format(p.map(|p| p.format).unwrap_or_default(), target_format) {
        Ok(f) => f,
        Err(e) => return e,
    };
    let row = size.width as usize * 4;
    let Some(mut pixels) = crate::filled(row * size.height as usize, 0u8) else {
        return E_OUTOFMEMORY;
    };
    if !src.is_null() {
        if (pitch as usize) < row {
            return E_INVALIDARG;
        }
        for y in 0..size.height as usize {
            // SAFETY: the caller's rows, `pitch` bytes apart.
            let line = unsafe { core::slice::from_raw_parts(src.add(y * pitch as usize), row) };
            pixels[y * row..(y + 1) * row].copy_from_slice(line);
        }
    }
    new_bitmap(factory, size.width, size.height, dpi_of(p), format, pixels, out)
}

// The WIC pixel formats a bitmap is made from.
const fn wic_format(last: u8) -> Guid {
    crate::guid(0x6fdd_c324, 0x4e03, 0x4bfe, [0xb1, 0x85, 0x3d, 0x77, 0x76, 0x8d, 0xc9, last])
}
pub const WIC_BGR32: Guid = wic_format(0x0e);
pub const WIC_PBGRA32: Guid = wic_format(0x10);
pub const WIC_RGB32: Guid =
    crate::guid(0xd98c_6b95, 0x3efe, 0x47d6, [0xbb, 0x25, 0xeb, 0x17, 0x48, 0xab, 0x0c, 0xf1]);
pub const WIC_PRGBA32: Guid =
    crate::guid(0x3cc4_a650, 0xa527, 0x4d37, [0xa9, 0x16, 0x31, 0x42, 0xc7, 0xeb, 0xed, 0xba]);

/// IWICBitmapSource's GetSize, GetPixelFormat and CopyPixels (slots 3,
/// 4 and 7) on any source.
pub fn wic_size(o: Obj) -> Result<(u32, u32), HResult> {
    let (mut w, mut h) = (0, 0);
    // SAFETY: IWICBitmapSource::GetSize's signature.
    let f: extern "system" fn(Obj, *mut u32, *mut u32) -> HResult =
        unsafe { core::mem::transmute(crate::slot(o, 3)) };
    match f(o, &mut w, &mut h) {
        S_OK => Ok((w, h)),
        e => Err(e),
    }
}

pub fn wic_format_of(o: Obj) -> Result<Guid, HResult> {
    let mut g = [0u8; 16];
    // SAFETY: IWICBitmapSource::GetPixelFormat's signature.
    let f: extern "system" fn(Obj, *mut Guid) -> HResult =
        unsafe { core::mem::transmute(crate::slot(o, 4)) };
    match f(o, &mut g) {
        S_OK => Ok(g),
        e => Err(e),
    }
}

fn wic_copy(o: Obj, stride: u32, buf: &mut [u8]) -> HResult {
    // SAFETY: IWICBitmapSource::CopyPixels's signature.
    let f: extern "system" fn(Obj, *const u8, u32, u32, *mut u8) -> HResult =
        unsafe { core::mem::transmute(crate::slot(o, 7)) };
    f(o, core::ptr::null(), stride, buf.len() as u32, buf.as_mut_ptr())
}

/// CreateBitmapFromWicBitmap: the source's pixels, which must be
/// 32bppPBGRA or 32bppBGR (alpha ignored); others
/// D2DERR_UNSUPPORTED_PIXEL_FORMAT, as Direct2D asks callers to convert
/// first (WICConvertBitmapSource).
pub fn create_from_wic(
    factory: Obj,
    source: Obj,
    props: *const BitmapProps,
    out: *mut Obj,
) -> HResult {
    if out.is_null() {
        return crate::E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if source.is_null() {
        return E_INVALIDARG;
    }
    let src_format = match wic_format_of(source) {
        Ok(f) => f,
        Err(e) => return e,
    };
    let natural = if src_format == WIC_PBGRA32 {
        ALPHA_PREMULTIPLIED
    } else if src_format == WIC_BGR32 {
        ALPHA_IGNORE
    } else {
        crate::gap(b"CreateBitmapFromWicBitmap(pixel format)", crate::data1(&src_format));
        return D2DERR_UNSUPPORTED_PIXEL_FORMAT;
    };
    let p = read(props);
    let format = match bitmap_format(
        p.map(|p| p.format).unwrap_or_default(),
        PixelFormat { format: DXGI_FORMAT_B8G8R8A8_UNORM, alpha: natural },
    ) {
        Ok(f) => f,
        Err(e) => return e,
    };
    let (w, h) = match wic_size(source) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let Some(mut pixels) = crate::filled(w as usize * h as usize * 4, 0u8) else {
        return E_OUTOFMEMORY;
    };
    if w > 0 && h > 0 {
        let r = wic_copy(source, w * 4, &mut pixels);
        if r != S_OK {
            return r;
        }
    }
    if natural == ALPHA_IGNORE || format.alpha == ALPHA_IGNORE {
        pixels.chunks_exact_mut(4).for_each(|p| p[3] = 255);
    }
    new_bitmap(factory, w, h, dpi_of(p), format, pixels, out)
}

extern "system" fn bitmap_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    query(this, b"ID2D1Bitmap", &[IID_RESOURCE, IID_IMAGE, IID_BITMAP], riid, out)
}

extern "system" fn bitmap_release(this: Obj) -> u32 {
    release_object::<Bitmap>(this)
}

/// GetSize: in DIPs (pixels at the bitmap's DPI). Returned through a
/// hidden pointer, as the C++ ABI returns structures from methods.
extern "system" fn bitmap_GetSize(this: Obj, ret: *mut SizeF) -> *mut SizeF {
    let b = bitmap(this);
    put(ret, SizeF { width: b.w as f32 * 96.0 / b.dpi.0, height: b.h as f32 * 96.0 / b.dpi.1 });
    ret
}

extern "system" fn bitmap_GetPixelSize(this: Obj, ret: *mut SizeU) -> *mut SizeU {
    let b = bitmap(this);
    put(ret, SizeU { width: b.w, height: b.h });
    ret
}

extern "system" fn bitmap_GetPixelFormat(this: Obj, ret: *mut PixelFormat) -> *mut PixelFormat {
    put(ret, bitmap(this).format);
    ret
}

extern "system" fn bitmap_GetDpi(this: Obj, x: *mut f32, y: *mut f32) {
    let b = bitmap(this);
    put(x, b.dpi.0);
    put(y, b.dpi.1);
}

/// D2D1_POINT_2U and D2D1_RECT_U.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct PointU {
    x: u32,
    y: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RectU {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

/// Copies `src_rect` (NULL: all of `sw` x `sh`) of a source to `dst` at
/// `at` within `this`; the rows come from `row(y)`.
fn copy_in(
    this: Obj,
    at: *const PointU,
    src_rect: *const RectU,
    sw: u32,
    sh: u32,
    row: impl Fn(u32) -> *const u8,
) -> HResult {
    let b = bitmap(this);
    let at = read(at).unwrap_or(PointU { x: 0, y: 0 });
    let r = read(src_rect).unwrap_or(RectU { left: 0, top: 0, right: sw, bottom: sh });
    if r.left > r.right || r.top > r.bottom || r.right > sw || r.bottom > sh {
        return E_INVALIDARG;
    }
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if u64::from(at.x) + u64::from(w) > u64::from(b.w)
        || u64::from(at.y) + u64::from(h) > u64::from(b.h)
    {
        return E_INVALIDARG;
    }
    b.pixels.with(|px| {
        for j in 0..h {
            let s = row(r.top + j);
            let d = ((at.y + j) * b.w + at.x) as usize * 4;
            // SAFETY: the source row holds at least `right` pixels.
            let line =
                unsafe { core::slice::from_raw_parts(s.add(r.left as usize * 4), w as usize * 4) };
            px[d..d + w as usize * 4].copy_from_slice(line);
        }
    });
    S_OK
}

extern "system" fn bitmap_CopyFromBitmap(
    this: Obj,
    at: *const PointU,
    other: Obj,
    src: *const RectU,
) -> HResult {
    if !is_bitmap(other) || other == this {
        return not_implemented(b"ID2D1Bitmap::CopyFromBitmap(not one of ours)");
    }
    let o = bitmap(other);
    // Snapshot the source so its lock is not held with ours.
    let Some(copy) = o.pixels.with(|p| {
        let mut v = Vec::new();
        v.try_reserve_exact(p.len()).ok()?;
        v.extend_from_slice(p);
        Some(v)
    }) else {
        return E_OUTOFMEMORY;
    };
    copy_in(this, at, src, o.w, o.h, |y| copy[(y * o.w * 4) as usize..].as_ptr())
}

extern "system" fn bitmap_CopyFromRenderTarget(
    _this: Obj,
    _at: *const PointU,
    _target: Obj,
    _src: *const RectU,
) -> HResult {
    not_implemented(b"ID2D1Bitmap::CopyFromRenderTarget")
}

extern "system" fn bitmap_CopyFromMemory(
    this: Obj,
    dst: *const RectU,
    src: *const u8,
    pitch: u32,
) -> HResult {
    if src.is_null() {
        return E_INVALIDARG;
    }
    let b = bitmap(this);
    let r = read(dst).unwrap_or(RectU { left: 0, top: 0, right: b.w, bottom: b.h });
    if r.left > r.right || r.top > r.bottom || r.right > b.w || r.bottom > b.h {
        return E_INVALIDARG;
    }
    let at = PointU { x: r.left, y: r.top };
    let whole = RectU { left: 0, top: 0, right: r.right - r.left, bottom: r.bottom - r.top };
    copy_in(this, &at, &whole, whole.right, whole.bottom, |y| {
        // SAFETY: the caller's rows, `pitch` bytes apart.
        unsafe { src.add(y as usize * pitch as usize) }
    })
}

// ---- Brushes --------------------------------------------------------------------

pub enum Kind {
    Solid(ColorF),
    Linear { start: Point, end: Point, stops: ComRef },
    Radial { center: Point, offset: Point, rx: f32, ry: f32, stops: ComRef },
    Bitmap { bitmap: ComRef, ex: u32, ey: u32, interp: u32 },
}

pub struct BrushState {
    pub opacity: f32,
    pub transform: Matrix,
    pub kind: Kind,
}

#[repr(C)]
pub struct Brush {
    res: Res,
    pub state: Spin<BrushState>,
}

/// D2D1_BRUSH_PROPERTIES.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BrushProps {
    pub opacity: f32,
    pub transform: Matrix,
}

macro_rules! brush_slots {
    ($($extra:expr),*) => {
        [
            brush_query as *const (),
            add_ref as *const (),
            brush_release as *const (),
            get_factory as *const (),
            brush_SetOpacity as *const (),
            brush_SetTransform as *const (),
            brush_GetOpacity as *const (),
            brush_GetTransform as *const (),
            $($extra as *const ()),*
        ]
    };
}

static SOLID_VTBL: VTable<10> = VTable(brush_slots!(solid_SetColor, solid_GetColor));
static LINEAR_VTBL: VTable<13> = VTable(brush_slots!(
    linear_SetStartPoint,
    linear_SetEndPoint,
    linear_GetStartPoint,
    linear_GetEndPoint,
    gradient_GetGradientStopCollection
));
static RADIAL_VTBL: VTable<17> = VTable(brush_slots!(
    radial_SetCenter,
    radial_SetGradientOriginOffset,
    radial_SetRadiusX,
    radial_SetRadiusY,
    radial_GetCenter,
    radial_GetGradientOriginOffset,
    radial_GetRadiusX,
    radial_GetRadiusY,
    gradient_GetGradientStopCollection
));
static BITMAP_BRUSH_VTBL: VTable<16> = VTable(brush_slots!(
    bb_SetExtendModeX,
    bb_SetExtendModeY,
    bb_SetInterpolationMode,
    bb_SetBitmap,
    bb_GetExtendModeX,
    bb_GetExtendModeY,
    bb_GetInterpolationMode,
    bb_GetBitmap
));

pub fn is_brush(o: Obj) -> bool {
    let v = vtable_of(o);
    [SOLID_VTBL.addr(), LINEAR_VTBL.addr(), RADIAL_VTBL.addr(), BITMAP_BRUSH_VTBL.addr()]
        .contains(&v)
}

fn brush<'a>(o: Obj) -> &'a Brush {
    // SAFETY: one of our brushes.
    unsafe { &*(o as *const Brush) }
}

/// A new brush of `kind` with the caller's properties (opacity 1 and
/// the identity transform when NULL).
pub fn create_brush(factory: Obj, kind: Kind, props: *const BrushProps, out: *mut Obj) -> HResult {
    if out.is_null() {
        return crate::E_POINTER;
    }
    let vtbl = match kind {
        Kind::Solid(_) => SOLID_VTBL.addr(),
        Kind::Linear { .. } => LINEAR_VTBL.addr(),
        Kind::Radial { .. } => RADIAL_VTBL.addr(),
        Kind::Bitmap { .. } => BITMAP_BRUSH_VTBL.addr(),
    };
    let p = read(props).unwrap_or(BrushProps { opacity: 1.0, transform: Matrix::IDENTITY });
    give(
        out,
        Brush {
            res: Res::new(vtbl, factory),
            state: Spin::new(BrushState { opacity: p.opacity, transform: p.transform, kind }),
        },
    )
}

extern "system" fn brush_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    let own = match vtable_of(this) {
        v if v == SOLID_VTBL.addr() => IID_SOLID_BRUSH,
        v if v == LINEAR_VTBL.addr() => IID_LINEAR_BRUSH,
        v if v == RADIAL_VTBL.addr() => IID_RADIAL_BRUSH,
        _ => IID_BITMAP_BRUSH,
    };
    query(this, b"ID2D1Brush", &[IID_RESOURCE, IID_BRUSH, own], riid, out)
}

extern "system" fn brush_release(this: Obj) -> u32 {
    release_object::<Brush>(this)
}

fn with_state<R>(this: Obj, f: impl FnOnce(&mut BrushState) -> R) -> R {
    brush(this).state.with(f)
}

extern "system" fn brush_SetOpacity(this: Obj, v: f32) {
    with_state(this, |s| s.opacity = v);
}

extern "system" fn brush_SetTransform(this: Obj, m: *const Matrix) {
    if let Some(m) = read(m) {
        with_state(this, |s| s.transform = m);
    }
}

extern "system" fn brush_GetOpacity(this: Obj) -> f32 {
    with_state(this, |s| s.opacity)
}

extern "system" fn brush_GetTransform(this: Obj, out: *mut Matrix) {
    put(out, with_state(this, |s| s.transform));
}

extern "system" fn solid_SetColor(this: Obj, c: *const ColorF) {
    if let Some(c) = read(c) {
        with_state(this, |s| s.kind = Kind::Solid(c));
    }
}

extern "system" fn solid_GetColor(this: Obj, ret: *mut ColorF) -> *mut ColorF {
    let c = with_state(this, |s| if let Kind::Solid(c) = s.kind { c } else { ColorF::default() });
    put(ret, c);
    ret
}

fn set_point(this: Obj, f: impl FnOnce(&mut Kind)) {
    with_state(this, |s| f(&mut s.kind));
}

extern "system" fn linear_SetStartPoint(this: Obj, p: Point) {
    set_point(this, |k| {
        if let Kind::Linear { start, .. } = k {
            *start = p
        }
    });
}

extern "system" fn linear_SetEndPoint(this: Obj, p: Point) {
    set_point(this, |k| {
        if let Kind::Linear { end, .. } = k {
            *end = p
        }
    });
}

fn get_point(this: Obj, ret: *mut Point, f: impl FnOnce(&Kind) -> Point) -> *mut Point {
    put(ret, with_state(this, |s| f(&s.kind)));
    ret
}

extern "system" fn linear_GetStartPoint(this: Obj, ret: *mut Point) -> *mut Point {
    get_point(
        this,
        ret,
        |k| if let Kind::Linear { start, .. } = k { *start } else { Point::default() },
    )
}

extern "system" fn linear_GetEndPoint(this: Obj, ret: *mut Point) -> *mut Point {
    get_point(this, ret, |k| if let Kind::Linear { end, .. } = k { *end } else { Point::default() })
}

extern "system" fn gradient_GetGradientStopCollection(this: Obj, out: *mut Obj) {
    let s = with_state(this, |s| match &s.kind {
        Kind::Linear { stops, .. } | Kind::Radial { stops, .. } => stops.0,
        _ => core::ptr::null_mut(),
    });
    if !s.is_null() {
        add_ref(s);
    }
    put(out, s);
}

extern "system" fn radial_SetCenter(this: Obj, p: Point) {
    set_point(this, |k| {
        if let Kind::Radial { center, .. } = k {
            *center = p
        }
    });
}

extern "system" fn radial_SetGradientOriginOffset(this: Obj, p: Point) {
    set_point(this, |k| {
        if let Kind::Radial { offset, .. } = k {
            *offset = p
        }
    });
}

extern "system" fn radial_SetRadiusX(this: Obj, v: f32) {
    set_point(this, |k| {
        if let Kind::Radial { rx, .. } = k {
            *rx = v
        }
    });
}

extern "system" fn radial_SetRadiusY(this: Obj, v: f32) {
    set_point(this, |k| {
        if let Kind::Radial { ry, .. } = k {
            *ry = v
        }
    });
}

extern "system" fn radial_GetCenter(this: Obj, ret: *mut Point) -> *mut Point {
    get_point(this, ret, |k| {
        if let Kind::Radial { center, .. } = k { *center } else { Point::default() }
    })
}

extern "system" fn radial_GetGradientOriginOffset(this: Obj, ret: *mut Point) -> *mut Point {
    get_point(this, ret, |k| {
        if let Kind::Radial { offset, .. } = k { *offset } else { Point::default() }
    })
}

extern "system" fn radial_GetRadiusX(this: Obj) -> f32 {
    with_state(this, |s| if let Kind::Radial { rx, .. } = s.kind { rx } else { 0.0 })
}

extern "system" fn radial_GetRadiusY(this: Obj) -> f32 {
    with_state(this, |s| if let Kind::Radial { ry, .. } = s.kind { ry } else { 0.0 })
}

extern "system" fn bb_SetExtendModeX(this: Obj, v: u32) {
    set_point(this, |k| {
        if let Kind::Bitmap { ex, .. } = k {
            *ex = v
        }
    });
}

extern "system" fn bb_SetExtendModeY(this: Obj, v: u32) {
    set_point(this, |k| {
        if let Kind::Bitmap { ey, .. } = k {
            *ey = v
        }
    });
}

extern "system" fn bb_SetInterpolationMode(this: Obj, v: u32) {
    set_point(this, |k| {
        if let Kind::Bitmap { interp, .. } = k {
            *interp = v
        }
    });
}

extern "system" fn bb_SetBitmap(this: Obj, b: Obj) {
    let new = ComRef::new(if is_bitmap(b) { b } else { core::ptr::null_mut() });
    // The old reference is dropped after the lock is let go.
    let old = with_state(this, |s| match &mut s.kind {
        Kind::Bitmap { bitmap, .. } => Some(core::mem::replace(bitmap, new)),
        _ => None,
    });
    drop(old);
}

extern "system" fn bb_GetExtendModeX(this: Obj) -> u32 {
    with_state(this, |s| if let Kind::Bitmap { ex, .. } = s.kind { ex } else { 0 })
}

extern "system" fn bb_GetExtendModeY(this: Obj) -> u32 {
    with_state(this, |s| if let Kind::Bitmap { ey, .. } = s.kind { ey } else { 0 })
}

extern "system" fn bb_GetInterpolationMode(this: Obj) -> u32 {
    with_state(this, |s| if let Kind::Bitmap { interp, .. } = s.kind { interp } else { 0 })
}

extern "system" fn bb_GetBitmap(this: Obj, out: *mut Obj) {
    let b = with_state(this, |s| match &s.kind {
        Kind::Bitmap { bitmap, .. } => bitmap.0,
        _ => core::ptr::null_mut(),
    });
    if !b.is_null() {
        add_ref(b);
    }
    put(out, b);
}

/// D2D1_BITMAP_BRUSH_PROPERTIES.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BitmapBrushProps {
    pub ex: u32,
    pub ey: u32,
    pub interp: u32,
}

/// The stop collection given to a gradient brush, if it is ours.
pub fn stops_ref(o: Obj) -> Option<ComRef> {
    is_stops(o).then(|| ComRef::new(o))
}

/// A bitmap's pixels as a paint reads them, through `f` (its lock held).
pub fn with_pixels<R>(b: &Bitmap, f: impl FnOnce(Pixels) -> R) -> R {
    b.pixels.with(|px| f(Pixels { data: px, w: b.w, h: b.h, stride: b.w as usize * 4 }))
}

/// Runs `f` with the paint of brush `o` for a target whose DIPs reach
/// device pixels through `world` (its transform and DPI scale). A brush
/// that is not ours, or whose transform cannot be inverted, paints
/// nothing (`f` is not called).
pub fn with_paint(o: Obj, world: &Matrix, f: impl FnOnce(&Paint)) {
    if !is_brush(o) {
        if !o.is_null() {
            crate::gap(b"draw(a brush not made by gala's d2d1)", 0);
        }
        return;
    }
    // A snapshot of the brush, so no lock is held while drawing.
    let (opacity, transform, kind) = with_state(o, |s| {
        let kind = match &s.kind {
            Kind::Solid(c) => Kind::Solid(*c),
            Kind::Linear { start, end, stops } => {
                Kind::Linear { start: *start, end: *end, stops: ComRef::new(stops.0) }
            }
            Kind::Radial { center, offset, rx, ry, stops } => Kind::Radial {
                center: *center,
                offset: *offset,
                rx: *rx,
                ry: *ry,
                stops: ComRef::new(stops.0),
            },
            Kind::Bitmap { bitmap, ex, ey, interp } => {
                Kind::Bitmap { bitmap: ComRef::new(bitmap.0), ex: *ex, ey: *ey, interp: *interp }
            }
        };
        (s.opacity, s.transform, kind)
    });
    let to_device = transform.then(world);
    match kind {
        Kind::Solid(c) => f(&Paint::Solid(Rgba::premultiplied(c, opacity))),
        Kind::Linear { start, end, stops } => {
            let Some(inv) = to_device.inverse() else { return };
            f(&Paint::Linear { inv, start, end, ramp: &self::stops(stops.0).ramp, opacity });
        }
        Kind::Radial { center, offset, rx, ry, stops } => {
            let Some(inv) = to_device.inverse() else { return };
            f(&Paint::Radial {
                inv,
                center,
                offset,
                rx,
                ry,
                ramp: &self::stops(stops.0).ramp,
                opacity,
            });
        }
        Kind::Bitmap { bitmap: b, ex, ey, interp } => {
            if b.0.is_null() {
                return;
            }
            let bm = bitmap(b.0);
            let px_to_dip = Matrix::scale(96.0 / bm.dpi.0, 96.0 / bm.dpi.1);
            let Some(inv) = px_to_dip.then(&to_device).inverse() else { return };
            if interp > 1 {
                crate::gap(b"ID2D1BitmapBrush(interpolation mode)", interp.into());
            }
            with_pixels(bm, |px| {
                f(&Paint::Bitmap { inv, px, ex, ey, linear: interp == 1, opacity })
            });
        }
    }
}

/// Runs `f` with the paint DrawBitmap fills `dst` with: the bitmap's
/// `src` (in its DIPs) stretched over `dst`, clamped at its edges.
pub fn with_bitmap_paint(
    b: &Bitmap,
    dst: &RectF,
    src: &RectF,
    world: &Matrix,
    opacity: f32,
    linear: bool,
    f: impl FnOnce(&Paint),
) {
    // Bitmap pixels -> source DIPs -> destination DIPs -> device.
    let (sx, sy) = (96.0 / b.dpi.0, 96.0 / b.dpi.1);
    let (sw, sh) = (src.right - src.left, src.bottom - src.top);
    if sw == 0.0 || sh == 0.0 {
        return;
    }
    let kx = (dst.right - dst.left) / sw;
    let ky = (dst.bottom - dst.top) / sh;
    let m = Matrix {
        m11: sx * kx,
        m12: 0.0,
        m21: 0.0,
        m22: sy * ky,
        dx: dst.left - src.left * kx,
        dy: dst.top - src.top * ky,
    };
    let Some(inv) = m.then(world).inverse() else { return };
    with_pixels(b, |px| {
        f(&Paint::Bitmap { inv, px, ex: EXTEND_CLAMP, ey: EXTEND_CLAMP, linear, opacity })
    });
}
