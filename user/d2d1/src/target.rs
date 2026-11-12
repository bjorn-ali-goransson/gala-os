//! Render targets: one object for the three kinds gala makes, told apart
//! by what backs their pixels.
//!
//! - WIC bitmap (CreateWicBitmapRenderTarget): the target draws into a
//!   buffer of its own, filled from the bitmap at BeginDraw and written
//!   back at EndDraw (IWICBitmap::Lock). 32bppPBGRA and 32bppBGR bitmaps
//!   are B8G8R8A8 targets, 32bppPRGBA and 32bppRGB R8G8B8A8 ones
//!   (premultiplied, or alpha ignored); asking for another format or
//!   alpha mode than the bitmap's is E_INVALIDARG (Wine's
//!   test_wic_target_format).
//! - DC (CreateDCRenderTarget, BindDC): a 32 bpp DIB section of the bound
//!   rectangle's size in a memory DC of its own. BeginDraw copies the
//!   DC's pixels in, EndDraw copies them back when something was drawn
//!   (so GDI's drawing between them survives an empty frame, Wine's
//!   test_dc_target); without a bound DC, EndDraw is D2DERR_WRONG_STATE.
//! - HWND (CreateHwndRenderTarget): a DIB section of the window's client
//!   size (or the size asked for) kept between frames; EndDraw copies it
//!   to the window's DC (GetDC, BitBlt).
//!
//! Drawing outside BeginDraw/EndDraw does nothing and makes the next
//! EndDraw D2DERR_WRONG_STATE; EndDraw without BeginDraw is
//! D2DERR_WRONG_STATE. The transform and the DPI scale (DPI / 96) take
//! DIPs to device pixels; clips are taken through the transform when
//! pushed. A request for hardware rendering (D2D1_RENDER_TARGET_TYPE_
//! HARDWARE) is drawn by the same software rasterizer: gala has no
//! Direct3D device (d3d11's D3D11CreateDevice fails), and this answers
//! as Windows' Direct2D does on a machine whose only adapter is the
//! Basic Render Driver (a CPU device); it is recorded in the ledger.

use alloc::vec::Vec;

use crate::brush::{self, BitmapBrushProps, BitmapProps, BrushProps, Kind, Stop};
use crate::geometry::{self, Ellipse, Path, RoundedRect};
use crate::raster::{self, Clip, IRect, Rgba, Surface};
use crate::{
    ALPHA_IGNORE, ALPHA_PREMULTIPLIED, ALPHA_STRAIGHT, ALPHA_UNKNOWN, ColorF, ComRef,
    D2DERR_PUSH_POP_UNBALANCED, D2DERR_UNSUPPORTED_PIXEL_FORMAT, D2DERR_WRONG_STATE,
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_UNKNOWN, E_INVALIDARG,
    E_OUTOFMEMORY, E_POINTER, Guid, HResult, IID_DC_RENDER_TARGET, IID_HWND_RENDER_TARGET,
    IID_RENDER_TARGET, IID_RESOURCE, Matrix, Obj, PixelFormat, Point, RectF, Res, S_OK, SizeF,
    SizeU, Spin, VTable, add_ref, gap, get_factory, give, not_implemented, pt, push, put, query,
    read, release_object, slot, vtable_of,
};

#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateCompatibleDC(hdc: usize) -> usize;
    fn DeleteDC(hdc: usize) -> i32;
    fn CreateDIBSection(
        hdc: usize,
        info: *const u8,
        usage: u32,
        bits: *mut *mut u8,
        section: usize,
        offset: u32,
    ) -> usize;
    fn SelectObject(hdc: usize, object: usize) -> usize;
    fn DeleteObject(object: usize) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn BitBlt(
        dst: usize,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        src: usize,
        sx: i32,
        sy: i32,
        rop: u32,
    ) -> i32;
    fn GetObjectType(h: usize) -> u32;
    fn GdiFlush() -> i32;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetDC(hwnd: usize) -> usize;
    fn ReleaseDC(hwnd: usize, hdc: usize) -> i32;
    fn IsWindow(hwnd: usize) -> i32;
    fn IsIconic(hwnd: usize) -> i32;
    fn GetClientRect(hwnd: usize, rect: *mut [i32; 4]) -> i32;
}

const SRCCOPY: u32 = 0x00cc_0020;
const OBJ_DC: u32 = 3;
const OBJ_MEMDC: u32 = 10;
const OBJ_ENHMETADC: u32 = 12;

/// D2D1_RENDER_TARGET_PROPERTIES.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TargetProps {
    kind: u32,
    format: PixelFormat,
    dpi_x: f32,
    dpi_y: f32,
    usage: u32,
    min_level: u32,
}

/// D2D1_HWND_RENDER_TARGET_PROPERTIES.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HwndProps {
    hwnd: usize,
    size: SizeU,
    present: u32,
}

const TYPE_HARDWARE: u32 = 2;

/// A 32 bpp top-down DIB section selected into a memory DC of our own.
struct Dib {
    dc: usize,
    bitmap: usize,
    old: usize,
    bits: *mut u8,
}

impl Dib {
    fn new(w: u32, h: u32) -> Option<Dib> {
        let mut info = [0u8; 44];
        info[0] = 40; // biSize
        info[4..8].copy_from_slice(&(w.max(1) as i32).to_le_bytes());
        info[8..12].copy_from_slice(&(-(h.max(1) as i32)).to_le_bytes());
        info[12] = 1; // biPlanes
        info[14] = 32; // biBitCount
        // SAFETY: plain GDI calls with our own header.
        unsafe {
            let dc = CreateCompatibleDC(0);
            if dc == 0 {
                return None;
            }
            let mut bits = core::ptr::null_mut();
            let bitmap = CreateDIBSection(dc, info.as_ptr(), 0, &mut bits, 0, 0);
            if bitmap == 0 || bits.is_null() {
                DeleteDC(dc);
                return None;
            }
            let old = SelectObject(dc, bitmap);
            Some(Dib { dc, bitmap, old, bits })
        }
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        // SAFETY: our own DC and bitmap.
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

enum Backing {
    Wic { bitmap: ComRef, buf: Vec<u8> },
    Dc { hdc: usize, origin: (i32, i32), dib: Option<Dib> },
    Hwnd { hwnd: usize, dib: Option<Dib> },
}

struct State {
    backing: Backing,
    w: u32,
    h: u32,
    format: PixelFormat,
    dpi: (f32, f32),
    transform: Matrix,
    antialias: u32,
    text_antialias: u32,
    text_params: ComRef,
    tags: (u64, u64),
    drawing: bool,
    error: HResult,
    error_tags: (u64, u64),
    clips: Vec<Clip>,
    layers: u32,
    dirty: bool,
}

// SAFETY: the raw pointers (the DIB's bits) belong to the target, used
// under its lock.
unsafe impl Send for State {}

#[repr(C)]
pub struct Target {
    res: Res,
    state: Spin<State>,
}

impl State {
    /// DIPs to device pixels.
    fn device(&self) -> Matrix {
        self.transform.then(&Matrix::scale(self.dpi.0 / 96.0, self.dpi.1 / 96.0))
    }

    fn surface(&mut self) -> Option<Surface> {
        let ptr = match &mut self.backing {
            Backing::Wic { buf, .. } => buf.as_mut_ptr(),
            Backing::Dc { dib: Some(d), .. } | Backing::Hwnd { dib: Some(d), .. } => d.bits,
            _ => return None,
        };
        if self.w == 0 || self.h == 0 {
            return None;
        }
        Some(Surface {
            ptr,
            w: self.w,
            h: self.h,
            stride: self.w as usize * 4,
            rgba: self.format.format == DXGI_FORMAT_R8G8B8A8_UNORM,
            opaque: self.format.alpha == ALPHA_IGNORE,
        })
    }

    fn clip(&self) -> Clip {
        let full = IRect { x0: 0, y0: 0, x1: self.w as i32, y1: self.h as i32 };
        match self.clips.last() {
            Some(c) => Clip { pixels: c.pixels.intersect(&full), exact: c.exact },
            None => Clip { pixels: full, exact: None },
        }
    }

    /// A draw call's check: outside BeginDraw/EndDraw it fails the next
    /// EndDraw.
    fn may_draw(&mut self) -> bool {
        if !self.drawing {
            if self.error == S_OK {
                self.error = D2DERR_WRONG_STATE;
                self.error_tags = self.tags;
            }
            return false;
        }
        true
    }
}

macro_rules! target_slots {
    ($($extra:expr),*) => {
        [
            t_query as *const (),
            add_ref as *const (),
            t_release as *const (),
            get_factory as *const (),
            t_CreateBitmap as *const (),
            t_CreateBitmapFromWicBitmap as *const (),
            t_CreateSharedBitmap as *const (),
            t_CreateBitmapBrush as *const (),
            t_CreateSolidColorBrush as *const (),
            t_CreateGradientStopCollection as *const (),
            t_CreateLinearGradientBrush as *const (),
            t_CreateRadialGradientBrush as *const (),
            t_CreateCompatibleRenderTarget as *const (),
            t_CreateLayer as *const (),
            t_CreateMesh as *const (),
            t_DrawLine as *const (),
            t_DrawRectangle as *const (),
            t_FillRectangle as *const (),
            t_DrawRoundedRectangle as *const (),
            t_FillRoundedRectangle as *const (),
            t_DrawEllipse as *const (),
            t_FillEllipse as *const (),
            t_DrawGeometry as *const (),
            t_FillGeometry as *const (),
            t_FillMesh as *const (),
            t_FillOpacityMask as *const (),
            t_DrawBitmap as *const (),
            t_DrawText as *const (),
            t_DrawTextLayout as *const (),
            t_DrawGlyphRun as *const (),
            t_SetTransform as *const (),
            t_GetTransform as *const (),
            t_SetAntialiasMode as *const (),
            t_GetAntialiasMode as *const (),
            t_SetTextAntialiasMode as *const (),
            t_GetTextAntialiasMode as *const (),
            t_SetTextRenderingParams as *const (),
            t_GetTextRenderingParams as *const (),
            t_SetTags as *const (),
            t_GetTags as *const (),
            t_PushLayer as *const (),
            t_PopLayer as *const (),
            t_Flush as *const (),
            t_SaveDrawingState as *const (),
            t_RestoreDrawingState as *const (),
            t_PushAxisAlignedClip as *const (),
            t_PopAxisAlignedClip as *const (),
            t_Clear as *const (),
            t_BeginDraw as *const (),
            t_EndDraw as *const (),
            t_GetPixelFormat as *const (),
            t_SetDpi as *const (),
            t_GetDpi as *const (),
            t_GetSize as *const (),
            t_GetPixelSize as *const (),
            t_GetMaximumBitmapSize as *const (),
            t_IsSupported as *const (),
            $($extra as *const ()),*
        ]
    };
}

static WIC_VTBL: VTable<57> = VTable(target_slots!());
static DC_VTBL: VTable<58> = VTable(target_slots!(dc_BindDC));
static HWND_VTBL: VTable<60> =
    VTable(target_slots!(hwnd_CheckWindowState, hwnd_Resize, hwnd_GetHwnd));

fn target<'a>(o: Obj) -> &'a Target {
    // SAFETY: one of our render targets.
    unsafe { &*(o as *const Target) }
}

fn with<R>(this: Obj, f: impl FnOnce(&mut State) -> R) -> R {
    target(this).state.with(f)
}

fn factory_of(this: Obj) -> Obj {
    target(this).res.factory.0
}

/// The DPI of a new target: both 0 is 96; one 0, or a negative one,
/// E_INVALIDARG (Wine's test_create_target).
fn dpi_of(p: &TargetProps) -> Result<(f32, f32), HResult> {
    match (p.dpi_x, p.dpi_y) {
        (x, y) if x == 0.0 && y == 0.0 => Ok((96.0, 96.0)),
        (x, y) if x > 0.0 && y > 0.0 => Ok((x, y)),
        _ => Err(E_INVALIDARG),
    }
}

fn note_type(p: &TargetProps) {
    if p.kind == TYPE_HARDWARE {
        gap(b"render target(hardware asked, drawn in software)", 0);
    }
}

fn new_state(backing: Backing, w: u32, h: u32, format: PixelFormat, dpi: (f32, f32)) -> State {
    State {
        backing,
        w,
        h,
        format,
        dpi,
        transform: Matrix::IDENTITY,
        antialias: 0,
        text_antialias: 0,
        text_params: ComRef(core::ptr::null_mut()),
        tags: (0, 0),
        drawing: false,
        error: S_OK,
        error_tags: (0, 0),
        clips: Vec::new(),
        layers: 0,
        dirty: false,
    }
}

fn make(factory: Obj, vtbl: *const (), state: State, out: *mut Obj) -> HResult {
    give(out, Target { res: Res::new(vtbl, factory), state: Spin::new(state) })
}

// ---- Creation (the factory's methods) ------------------------------------------

/// CreateWicBitmapRenderTarget.
pub extern "system" fn f_CreateWicBitmapRenderTarget(
    factory: Obj,
    bitmap: Obj,
    props: *const TargetProps,
    out: *mut Obj,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let Some(p) = read(props) else { return E_INVALIDARG };
    if bitmap.is_null() {
        return E_INVALIDARG;
    }
    let dpi = match dpi_of(&p) {
        Ok(d) => d,
        Err(e) => return e,
    };
    note_type(&p);
    let wic = match brush::wic_format_of(bitmap) {
        Ok(f) => f,
        Err(e) => return e,
    };
    let natural = if wic == brush::WIC_PBGRA32 {
        PixelFormat { format: DXGI_FORMAT_B8G8R8A8_UNORM, alpha: ALPHA_PREMULTIPLIED }
    } else if wic == brush::WIC_BGR32 {
        PixelFormat { format: DXGI_FORMAT_B8G8R8A8_UNORM, alpha: ALPHA_IGNORE }
    } else if wic == brush::WIC_PRGBA32 {
        PixelFormat { format: DXGI_FORMAT_R8G8B8A8_UNORM, alpha: ALPHA_PREMULTIPLIED }
    } else if wic == brush::WIC_RGB32 {
        PixelFormat { format: DXGI_FORMAT_R8G8B8A8_UNORM, alpha: ALPHA_IGNORE }
    } else {
        gap(b"CreateWicBitmapRenderTarget(pixel format)", crate::data1(&wic));
        return D2DERR_UNSUPPORTED_PIXEL_FORMAT;
    };
    if (p.format.format != DXGI_FORMAT_UNKNOWN && p.format.format != natural.format)
        || (p.format.alpha != ALPHA_UNKNOWN && p.format.alpha != natural.alpha)
    {
        return E_INVALIDARG;
    }
    let (w, h) = match brush::wic_size(bitmap) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let Some(buf) = crate::filled(w as usize * h as usize * 4, 0u8) else {
        return E_OUTOFMEMORY;
    };
    let backing = Backing::Wic { bitmap: ComRef::new(bitmap), buf };
    make(factory, WIC_VTBL.addr(), new_state(backing, w, h, natural, dpi), out)
}

/// CreateDCRenderTarget: B8G8R8A8, premultiplied or ignoring alpha;
/// anything else D2DERR_UNSUPPORTED_PIXEL_FORMAT (Wine's test_dc_target).
/// Its size is 0 x 0 until BindDC.
pub extern "system" fn f_CreateDCRenderTarget(
    factory: Obj,
    props: *const TargetProps,
    out: *mut Obj,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let Some(p) = read(props) else { return E_INVALIDARG };
    if p.format.format != DXGI_FORMAT_B8G8R8A8_UNORM
        || (p.format.alpha != ALPHA_PREMULTIPLIED && p.format.alpha != ALPHA_IGNORE)
    {
        return D2DERR_UNSUPPORTED_PIXEL_FORMAT;
    }
    let dpi = match dpi_of(&p) {
        Ok(d) => d,
        Err(e) => return e,
    };
    note_type(&p);
    let backing = Backing::Dc { hdc: 0, origin: (0, 0), dib: None };
    make(factory, DC_VTBL.addr(), new_state(backing, 0, 0, p.format, dpi), out)
}

/// CreateHwndRenderTarget: B8G8R8A8 (UNKNOWN is that), alpha
/// premultiplied or ignored (UNKNOWN is ignored); straight alpha or
/// another format D2DERR_UNSUPPORTED_PIXEL_FORMAT (Wine's
/// test_hwnd_target); not a window E_INVALIDARG.
pub extern "system" fn f_CreateHwndRenderTarget(
    factory: Obj,
    props: *const TargetProps,
    hprops: *const HwndProps,
    out: *mut Obj,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let (Some(p), Some(hp)) = (read(props), read(hprops)) else { return E_INVALIDARG };
    // SAFETY: a plain user32 query.
    if hp.hwnd == 0 || unsafe { IsWindow(hp.hwnd) } == 0 {
        return E_INVALIDARG;
    }
    let format = match p.format.format {
        DXGI_FORMAT_UNKNOWN | DXGI_FORMAT_B8G8R8A8_UNORM => DXGI_FORMAT_B8G8R8A8_UNORM,
        _ => return D2DERR_UNSUPPORTED_PIXEL_FORMAT,
    };
    let alpha = match p.format.alpha {
        ALPHA_UNKNOWN | ALPHA_IGNORE => ALPHA_IGNORE,
        ALPHA_PREMULTIPLIED => ALPHA_PREMULTIPLIED,
        _ => return D2DERR_UNSUPPORTED_PIXEL_FORMAT,
    };
    let dpi = match dpi_of(&p) {
        Ok(d) => d,
        Err(e) => return e,
    };
    note_type(&p);
    let (w, h) = window_size(hp.hwnd, hp.size);
    let Some(dib) = Dib::new(w, h) else { return E_OUTOFMEMORY };
    let backing = Backing::Hwnd { hwnd: hp.hwnd, dib: Some(dib) };
    make(
        factory,
        HWND_VTBL.addr(),
        new_state(backing, w, h, PixelFormat { format, alpha }, dpi),
        out,
    )
}

/// The size asked for, or the window's client size when it is 0 x 0.
fn window_size(hwnd: usize, size: SizeU) -> (u32, u32) {
    if size.width != 0 && size.height != 0 {
        return (size.width, size.height);
    }
    let mut r = [0i32; 4];
    // SAFETY: a plain user32 query into our buffer.
    unsafe { GetClientRect(hwnd, &mut r) };
    ((r[2] - r[0]).max(0) as u32, (r[3] - r[1]).max(0) as u32)
}

// ---- IUnknown -------------------------------------------------------------------

extern "system" fn t_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    let v = vtable_of(this);
    let own = if v == DC_VTBL.addr() {
        IID_DC_RENDER_TARGET
    } else if v == HWND_VTBL.addr() {
        IID_HWND_RENDER_TARGET
    } else {
        IID_RENDER_TARGET
    };
    query(this, b"ID2D1RenderTarget", &[IID_RESOURCE, IID_RENDER_TARGET, own], riid, out)
}

extern "system" fn t_release(this: Obj) -> u32 {
    release_object::<Target>(this)
}

// ---- Resources ---------------------------------------------------------------------

extern "system" fn t_CreateBitmap(
    this: Obj,
    size: SizeU,
    src: *const u8,
    pitch: u32,
    props: *const BitmapProps,
    out: *mut Obj,
) -> HResult {
    let format = with(this, |s| s.format);
    brush::create_bitmap(factory_of(this), format, size, src, pitch, props, out)
}

extern "system" fn t_CreateBitmapFromWicBitmap(
    this: Obj,
    source: Obj,
    props: *const BitmapProps,
    out: *mut Obj,
) -> HResult {
    brush::create_from_wic(factory_of(this), source, props, out)
}

extern "system" fn t_CreateSharedBitmap(
    _this: Obj,
    _iid: *const Guid,
    _data: Obj,
    _props: *const BitmapProps,
    out: *mut Obj,
) -> HResult {
    put(out, core::ptr::null_mut());
    not_implemented(b"ID2D1RenderTarget::CreateSharedBitmap")
}

/// CreateBitmapBrush: clamped, linear by default (d2d1.h's
/// D2D1::BitmapBrushProperties()); a bitmap not made by this DLL is
/// E_INVALIDARG.
extern "system" fn t_CreateBitmapBrush(
    this: Obj,
    bitmap: Obj,
    bprops: *const BitmapBrushProps,
    props: *const BrushProps,
    out: *mut Obj,
) -> HResult {
    if !bitmap.is_null() && !brush::is_bitmap(bitmap) {
        put(out, core::ptr::null_mut());
        return E_INVALIDARG;
    }
    let b = read(bprops).unwrap_or(BitmapBrushProps { ex: 0, ey: 0, interp: 1 });
    let kind = Kind::Bitmap { bitmap: ComRef::new(bitmap), ex: b.ex, ey: b.ey, interp: b.interp };
    brush::create_brush(factory_of(this), kind, props, out)
}

extern "system" fn t_CreateSolidColorBrush(
    this: Obj,
    color: *const ColorF,
    props: *const BrushProps,
    out: *mut Obj,
) -> HResult {
    let Some(c) = read(color) else {
        put(out, core::ptr::null_mut());
        return E_INVALIDARG;
    };
    brush::create_brush(factory_of(this), Kind::Solid(c), props, out)
}

extern "system" fn t_CreateGradientStopCollection(
    this: Obj,
    stops: *const Stop,
    n: u32,
    gamma: u32,
    extend: u32,
    out: *mut Obj,
) -> HResult {
    brush::create_stops(factory_of(this), stops, n, gamma, extend, out)
}

extern "system" fn t_CreateLinearGradientBrush(
    this: Obj,
    lprops: *const [Point; 2],
    props: *const BrushProps,
    stops: Obj,
    out: *mut Obj,
) -> HResult {
    let (Some(l), Some(stops)) = (read(lprops), brush::stops_ref(stops)) else {
        put(out, core::ptr::null_mut());
        return E_INVALIDARG;
    };
    brush::create_brush(
        factory_of(this),
        Kind::Linear { start: l[0], end: l[1], stops },
        props,
        out,
    )
}

/// D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES.
#[repr(C)]
#[derive(Clone, Copy)]
struct RadialProps {
    center: Point,
    offset: Point,
    rx: f32,
    ry: f32,
}

extern "system" fn t_CreateRadialGradientBrush(
    this: Obj,
    rprops: *const RadialProps,
    props: *const BrushProps,
    stops: Obj,
    out: *mut Obj,
) -> HResult {
    let (Some(r), Some(stops)) = (read(rprops), brush::stops_ref(stops)) else {
        put(out, core::ptr::null_mut());
        return E_INVALIDARG;
    };
    let kind = Kind::Radial { center: r.center, offset: r.offset, rx: r.rx, ry: r.ry, stops };
    brush::create_brush(factory_of(this), kind, props, out)
}

extern "system" fn t_CreateCompatibleRenderTarget(
    _this: Obj,
    _size: *const SizeF,
    _pixel_size: *const SizeU,
    _format: *const PixelFormat,
    _options: u32,
    out: *mut Obj,
) -> HResult {
    put(out, core::ptr::null_mut());
    not_implemented(b"ID2D1RenderTarget::CreateCompatibleRenderTarget")
}

extern "system" fn t_CreateLayer(_this: Obj, _size: *const SizeF, out: *mut Obj) -> HResult {
    put(out, core::ptr::null_mut());
    not_implemented(b"ID2D1RenderTarget::CreateLayer")
}

extern "system" fn t_CreateMesh(_this: Obj, out: *mut Obj) -> HResult {
    put(out, core::ptr::null_mut());
    not_implemented(b"ID2D1RenderTarget::CreateMesh")
}

// ---- Drawing --------------------------------------------------------------------------

/// Fills `path` (built from DIPs) with `brush`.
fn fill_path(this: Obj, path: Option<Path>, brush_obj: Obj) {
    with(this, |s| {
        if !s.may_draw() {
            return;
        }
        let Some(path) = path else { return };
        let device = s.device();
        let clip = s.clip();
        let aa = s.antialias == 0;
        let Some(polys) = path.fill_polys(&device) else { return };
        let Some(mask) = raster::coverage(&polys, path.winding, aa, &clip) else { return };
        if mask.cov.is_empty() {
            return;
        }
        let Some(mut surf) = s.surface() else { return };
        brush::with_paint(brush_obj, &device, |p| surf.compose(&mask, p));
        s.dirty = true;
    });
}

/// Strokes `path` `width` DIPs wide.
fn stroke_path(this: Obj, path: Option<Path>, brush_obj: Obj, width: f32, style: Obj) {
    let st = crate::factory::stroke_style_of(style);
    with(this, |s| {
        if !s.may_draw() {
            return;
        }
        let Some(path) = path else { return };
        let device = s.device();
        let clip = s.clip();
        let aa = s.antialias == 0;
        let Some(figs) = path.flatten(&device) else { return };
        let Some(polys) = raster::stroke(&figs, width * device.max_scale(), &st) else { return };
        let Some(mask) = raster::coverage(&polys, true, aa, &clip) else { return };
        if mask.cov.is_empty() {
            return;
        }
        let Some(mut surf) = s.surface() else { return };
        brush::with_paint(brush_obj, &device, |p| surf.compose(&mask, p));
        s.dirty = true;
    });
}

extern "system" fn t_DrawLine(this: Obj, a: Point, b: Point, brush: Obj, width: f32, style: Obj) {
    stroke_path(this, geometry::line_path(a, b), brush, width, style);
}

extern "system" fn t_DrawRectangle(this: Obj, r: *const RectF, brush: Obj, width: f32, style: Obj) {
    let path = read(r).and_then(|r| geometry::rect_path(&r));
    stroke_path(this, path, brush, width, style);
}

extern "system" fn t_FillRectangle(this: Obj, r: *const RectF, brush: Obj) {
    fill_path(this, read(r).and_then(|r| geometry::rect_path(&r)), brush);
}

extern "system" fn t_DrawRoundedRectangle(
    this: Obj,
    r: *const RoundedRect,
    brush: Obj,
    width: f32,
    style: Obj,
) {
    let path = read(r).and_then(|r| geometry::rounded_path(&r.rect, r.rx, r.ry));
    stroke_path(this, path, brush, width, style);
}

extern "system" fn t_FillRoundedRectangle(this: Obj, r: *const RoundedRect, brush: Obj) {
    fill_path(this, read(r).and_then(|r| geometry::rounded_path(&r.rect, r.rx, r.ry)), brush);
}

extern "system" fn t_DrawEllipse(this: Obj, e: *const Ellipse, brush: Obj, width: f32, style: Obj) {
    let path = read(e).and_then(|e| geometry::ellipse_path(e.center, e.rx, e.ry));
    stroke_path(this, path, brush, width, style);
}

extern "system" fn t_FillEllipse(this: Obj, e: *const Ellipse, brush: Obj) {
    fill_path(this, read(e).and_then(|e| geometry::ellipse_path(e.center, e.rx, e.ry)), brush);
}

/// A copy of geometry `g`'s path (None when it is not ours, or not
/// closed: recorded).
fn path_of(g: Obj) -> Option<Path> {
    if !geometry::is_geometry(g) {
        if !g.is_null() {
            gap(b"draw(a geometry not made by gala's d2d1)", 0);
        }
        return None;
    }
    geometry::with_path(g, |p| {
        let mut figs = Vec::new();
        figs.try_reserve_exact(p.figs.len()).ok()?;
        for f in &p.figs {
            let mut segs = Vec::new();
            segs.try_reserve_exact(f.segs.len()).ok()?;
            segs.extend_from_slice(&f.segs);
            figs.push(geometry::Fig { start: f.start, segs, closed: f.closed, filled: f.filled });
        }
        Some(Path { figs, winding: p.winding })
    })
    .ok()
    .flatten()
}

extern "system" fn t_DrawGeometry(this: Obj, g: Obj, brush: Obj, width: f32, style: Obj) {
    stroke_path(this, path_of(g), brush, width, style);
}

extern "system" fn t_FillGeometry(this: Obj, g: Obj, brush: Obj, opacity_brush: Obj) {
    if !opacity_brush.is_null() {
        gap(b"ID2D1RenderTarget::FillGeometry(opacity brush)", 0);
    }
    fill_path(this, path_of(g), brush);
}

extern "system" fn t_FillMesh(_this: Obj, _mesh: Obj, _brush: Obj) {
    gap(b"ID2D1RenderTarget::FillMesh", 0);
}

extern "system" fn t_FillOpacityMask(
    _this: Obj,
    _mask: Obj,
    _brush: Obj,
    _content: u32,
    _dst: *const RectF,
    _src: *const RectF,
) {
    gap(b"ID2D1RenderTarget::FillOpacityMask", 0);
}

/// DrawBitmap: the source rectangle (all of the bitmap by default, in its
/// DIPs) stretched over the destination (the source's size at the origin
/// by default), nearest-neighbour (0) or linear (1).
extern "system" fn t_DrawBitmap(
    this: Obj,
    bitmap: Obj,
    dst: *const RectF,
    opacity: f32,
    interp: u32,
    src: *const RectF,
) {
    if !brush::is_bitmap(bitmap) {
        if !bitmap.is_null() {
            gap(b"DrawBitmap(a bitmap not made by gala's d2d1)", 0);
        }
        return;
    }
    let b = brush::bitmap(bitmap);
    let whole = RectF {
        left: 0.0,
        top: 0.0,
        right: b.w as f32 * 96.0 / b.dpi.0,
        bottom: b.h as f32 * 96.0 / b.dpi.1,
    };
    let src = read(src).unwrap_or(whole);
    let dst = read(dst).unwrap_or(RectF {
        left: 0.0,
        top: 0.0,
        right: src.right - src.left,
        bottom: src.bottom - src.top,
    });
    with(this, |s| {
        if !s.may_draw() {
            return;
        }
        let device = s.device();
        let clip = s.clip();
        let aa = s.antialias == 0;
        let Some(path) = geometry::rect_path(&dst) else { return };
        let Some(polys) = path.fill_polys(&device) else { return };
        let Some(mask) = raster::coverage(&polys, false, aa, &clip) else { return };
        if mask.cov.is_empty() {
            return;
        }
        let Some(mut surf) = s.surface() else { return };
        brush::with_bitmap_paint(b, &dst, &src, &device, opacity, interp == 1, |p| {
            surf.compose(&mask, p)
        });
        s.dirty = true;
    });
}

extern "system" fn t_DrawText(
    _this: Obj,
    _text: *const u16,
    length: u32,
    _format: Obj,
    _rect: *const RectF,
    _brush: Obj,
    _options: u32,
    _measuring: u32,
) {
    gap(b"ID2D1RenderTarget::DrawText(no text layout)", length.into());
}

extern "system" fn t_DrawTextLayout(
    _this: Obj,
    _origin: Point,
    _layout: Obj,
    _brush: Obj,
    _options: u32,
) {
    gap(b"ID2D1RenderTarget::DrawTextLayout(no text layout)", 0);
}

extern "system" fn t_DrawGlyphRun(
    _this: Obj,
    _origin: Point,
    _run: *const u8,
    _brush: Obj,
    _mode: u32,
) {
    gap(b"ID2D1RenderTarget::DrawGlyphRun", 0);
}

// ---- State ------------------------------------------------------------------------------

extern "system" fn t_SetTransform(this: Obj, m: *const Matrix) {
    if let Some(m) = read(m) {
        with(this, |s| s.transform = m);
    }
}

extern "system" fn t_GetTransform(this: Obj, out: *mut Matrix) {
    put(out, with(this, |s| s.transform));
}

extern "system" fn t_SetAntialiasMode(this: Obj, mode: u32) {
    with(this, |s| s.antialias = mode);
}

extern "system" fn t_GetAntialiasMode(this: Obj) -> u32 {
    with(this, |s| s.antialias)
}

extern "system" fn t_SetTextAntialiasMode(this: Obj, mode: u32) {
    with(this, |s| s.text_antialias = mode);
}

extern "system" fn t_GetTextAntialiasMode(this: Obj) -> u32 {
    with(this, |s| s.text_antialias)
}

extern "system" fn t_SetTextRenderingParams(this: Obj, params: Obj) {
    let new = ComRef::new(params);
    let old = with(this, |s| core::mem::replace(&mut s.text_params, new));
    drop(old);
}

extern "system" fn t_GetTextRenderingParams(this: Obj, out: *mut Obj) {
    let p = with(this, |s| s.text_params.0);
    if !p.is_null() {
        crate::com_add_ref(p);
    }
    put(out, p);
}

extern "system" fn t_SetTags(this: Obj, a: u64, b: u64) {
    with(this, |s| s.tags = (a, b));
}

extern "system" fn t_GetTags(this: Obj, a: *mut u64, b: *mut u64) {
    let t = with(this, |s| s.tags);
    put(a, t.0);
    put(b, t.1);
}

/// PushLayer: the layer's mask, geometry and opacity are not applied
/// (recorded); the push is counted so PopLayer balances.
extern "system" fn t_PushLayer(this: Obj, _params: *const u8, _layer: Obj) {
    gap(b"ID2D1RenderTarget::PushLayer(content not masked)", 0);
    with(this, |s| s.layers += 1);
}

extern "system" fn t_PopLayer(this: Obj) {
    with(this, |s| s.layers = s.layers.saturating_sub(1));
}

extern "system" fn t_Flush(this: Obj, a: *mut u64, b: *mut u64) -> HResult {
    let (e, t) = with(this, |s| (s.error, s.error_tags));
    put(a, t.0);
    put(b, t.1);
    e
}

extern "system" fn t_SaveDrawingState(_this: Obj, _block: Obj) {
    gap(b"ID2D1RenderTarget::SaveDrawingState", 0);
}

extern "system" fn t_RestoreDrawingState(_this: Obj, _block: Obj) {
    gap(b"ID2D1RenderTarget::RestoreDrawingState", 0);
}

/// PushAxisAlignedClip: the rectangle through the current transform (its
/// bounding box), intersected with the clip below it; aliased clips are
/// rounded to whole pixels.
extern "system" fn t_PushAxisAlignedClip(this: Obj, r: *const RectF, mode: u32) {
    let Some(r) = read(r) else { return };
    with(this, |s| {
        let m = s.device();
        let corners = [
            m.apply(pt(r.left, r.top)),
            m.apply(pt(r.right, r.top)),
            m.apply(pt(r.right, r.bottom)),
            m.apply(pt(r.left, r.bottom)),
        ];
        let mut b = RectF { left: f32::MAX, top: f32::MAX, right: f32::MIN, bottom: f32::MIN };
        for c in corners {
            b.left = b.left.min(c.x);
            b.top = b.top.min(c.y);
            b.right = b.right.max(c.x);
            b.bottom = b.bottom.max(c.y);
        }
        let below = s.clip();
        let clip = if mode == 1 {
            let round = |v: f32| libm::floorf(v + 0.5) as i32;
            let px = IRect {
                x0: round(b.left),
                y0: round(b.top),
                x1: round(b.right),
                y1: round(b.bottom),
            };
            Clip { pixels: px.intersect(&below.pixels), exact: below.exact }
        } else {
            let px = IRect {
                x0: libm::floorf(b.left) as i32,
                y0: libm::floorf(b.top) as i32,
                x1: libm::ceilf(b.right) as i32,
                y1: libm::ceilf(b.bottom) as i32,
            };
            let exact = match below.exact {
                Some(e) => RectF {
                    left: e.left.max(b.left),
                    top: e.top.max(b.top),
                    right: e.right.min(b.right),
                    bottom: e.bottom.min(b.bottom),
                },
                None => b,
            };
            Clip { pixels: px.intersect(&below.pixels), exact: Some(exact) }
        };
        if !push(&mut s.clips, clip) && s.error == S_OK {
            s.error = E_OUTOFMEMORY;
        }
    });
}

extern "system" fn t_PopAxisAlignedClip(this: Obj) {
    with(this, |s| {
        s.clips.pop();
    });
}

/// Clear: the colour (transparent black for NULL) replaces what is inside
/// the clip; the transform does not apply.
extern "system" fn t_Clear(this: Obj, c: *const ColorF) {
    let c = read(c).unwrap_or_default();
    with(this, |s| {
        if !s.may_draw() {
            return;
        }
        let clip = s.clip();
        let Some(mut surf) = s.surface() else { return };
        surf.fill(&clip, Rgba::premultiplied(c, 1.0));
        s.dirty = true;
    });
}

// IWICBitmap::Lock (slot 8) and IWICBitmapLock's GetStride (4) and
// GetDataPointer (5).
type FnLock = extern "system" fn(Obj, *const [i32; 4], u32, *mut Obj) -> HResult;
type FnStride = extern "system" fn(Obj, *mut u32) -> HResult;
type FnData = extern "system" fn(Obj, *mut u32, *mut *mut u8) -> HResult;

/// Runs `f` on a WIC bitmap's pixels (`w` x `h`, 4 bytes each) through a
/// lock of `flags` (1 read, 2 write).
fn with_wic_pixels(
    bitmap: Obj,
    w: u32,
    h: u32,
    flags: u32,
    f: impl FnOnce(*mut u8, usize),
) -> HResult {
    let rect = [0, 0, w as i32, h as i32];
    let mut lock = core::ptr::null_mut();
    // SAFETY: IWICBitmap::Lock's signature.
    let lock_fn: FnLock = unsafe { core::mem::transmute(slot(bitmap, 8)) };
    let r = lock_fn(bitmap, &rect, flags, &mut lock);
    if r != S_OK {
        return r;
    }
    let (mut stride, mut size, mut data) = (0u32, 0u32, core::ptr::null_mut());
    // SAFETY: IWICBitmapLock's signatures.
    let (stride_fn, data_fn): (FnStride, FnData) = unsafe {
        (
            core::mem::transmute::<*const (), FnStride>(slot(lock, 4)),
            core::mem::transmute::<*const (), FnData>(slot(lock, 5)),
        )
    };
    let mut r = stride_fn(lock, &mut stride);
    if r == S_OK {
        r = data_fn(lock, &mut size, &mut data);
    }
    let need = stride as usize * (h as usize).saturating_sub(1) + w as usize * 4;
    if r == S_OK && !data.is_null() && (size as usize) >= need && stride as usize >= w as usize * 4
    {
        f(data, stride as usize);
    } else if r == S_OK {
        r = E_INVALIDARG;
    }
    crate::com_release(lock);
    r
}

extern "system" fn t_BeginDraw(this: Obj) {
    // A WIC target reads its bitmap first, without our lock held.
    let wic = with(this, |s| match &s.backing {
        Backing::Wic { bitmap, .. } => Some((ComRef::new(bitmap.0), s.w, s.h)),
        _ => None,
    });
    if let Some((bitmap, w, h)) = wic {
        let mut snapshot: Vec<u8> = Vec::new();
        let r = if snapshot.try_reserve_exact(w as usize * h as usize * 4).is_err() {
            E_OUTOFMEMORY
        } else {
            with_wic_pixels(bitmap.0, w, h, 1, |data, stride| {
                for y in 0..h as usize {
                    // SAFETY: inside the lock's rows.
                    let line = unsafe {
                        core::slice::from_raw_parts(data.add(y * stride), w as usize * 4)
                    };
                    snapshot.extend_from_slice(line);
                }
            })
        };
        with(this, |s| {
            if let Backing::Wic { buf, .. } = &mut s.backing
                && r == S_OK
                && snapshot.len() == buf.len()
            {
                buf.copy_from_slice(&snapshot);
            }
            if r != S_OK && s.error == S_OK {
                gap(b"BeginDraw(WIC bitmap lock)", r as u32 as u64);
                s.error = r;
            }
        });
    }
    with(this, |s| {
        if s.drawing && s.error == S_OK {
            s.error = D2DERR_WRONG_STATE;
            s.error_tags = s.tags;
        }
        s.drawing = true;
        s.dirty = false;
        match &s.backing {
            Backing::Dc { hdc: 0, .. } => {
                // No DC bound: the frame fails at EndDraw (tags 0).
                if s.error == S_OK {
                    s.error = D2DERR_WRONG_STATE;
                    s.error_tags = (0, 0);
                }
            }
            Backing::Dc { hdc, origin, dib: Some(d) } => {
                // SAFETY: the bound DC and our DIB's DC.
                unsafe {
                    BitBlt(d.dc, 0, 0, s.w as i32, s.h as i32, *hdc, origin.0, origin.1, SRCCOPY);
                    GdiFlush();
                }
            }
            _ => {}
        }
    });
}

extern "system" fn t_EndDraw(this: Obj, tag1: *mut u64, tag2: *mut u64) -> HResult {
    put(tag1, 0);
    put(tag2, 0);
    let (r, tags, wic) = with(this, |s| {
        if !s.drawing {
            return (D2DERR_WRONG_STATE, (0, 0), None);
        }
        s.drawing = false;
        if !s.clips.is_empty() || s.layers != 0 {
            s.clips.clear();
            s.layers = 0;
            if s.error == S_OK {
                s.error = D2DERR_PUSH_POP_UNBALANCED;
                s.error_tags = s.tags;
            }
        }
        let e = core::mem::replace(&mut s.error, S_OK);
        let t = core::mem::take(&mut s.error_tags);
        if e != S_OK {
            return (e, t, None);
        }
        let dirty = core::mem::take(&mut s.dirty);
        let (w, h) = (s.w as i32, s.h as i32);
        match &s.backing {
            Backing::Dc { hdc, origin, dib: Some(d) } if dirty => {
                // SAFETY: our DIB's DC and the bound DC.
                unsafe {
                    GdiFlush();
                    BitBlt(*hdc, origin.0, origin.1, w, h, d.dc, 0, 0, SRCCOPY);
                }
            }
            Backing::Hwnd { hwnd, dib: Some(d) } => {
                // SAFETY: the window's DC for the copy, released after.
                unsafe {
                    GdiFlush();
                    let dc = GetDC(*hwnd);
                    if dc != 0 {
                        BitBlt(dc, 0, 0, w, h, d.dc, 0, 0, SRCCOPY);
                        ReleaseDC(*hwnd, dc);
                    }
                }
            }
            Backing::Wic { bitmap, .. } if dirty => {
                return (S_OK, (0, 0), Some((ComRef::new(bitmap.0), s.w, s.h)));
            }
            _ => {}
        }
        (S_OK, (0, 0), None)
    });
    if let Some((bitmap, w, h)) = wic {
        // Write the frame back, our lock taken only for the copy.
        let r = with_wic_pixels(bitmap.0, w, h, 2, |data, stride| {
            with(this, |s| {
                if let Backing::Wic { buf, .. } = &s.backing {
                    for y in 0..h as usize {
                        let row = w as usize * 4;
                        // SAFETY: inside the lock's rows.
                        let line =
                            unsafe { core::slice::from_raw_parts_mut(data.add(y * stride), row) };
                        line.copy_from_slice(&buf[y * row..(y + 1) * row]);
                    }
                }
            });
        });
        if r != S_OK {
            gap(b"EndDraw(WIC bitmap lock)", r as u32 as u64);
            return r;
        }
    }
    put(tag1, tags.0);
    put(tag2, tags.1);
    r
}

extern "system" fn t_GetPixelFormat(this: Obj, ret: *mut PixelFormat) -> *mut PixelFormat {
    put(ret, with(this, |s| s.format));
    ret
}

/// SetDpi: 0, 0 is 96 (the desktop's); a negative value, or one 0, leaves
/// it as it is (Wine's test_clip).
extern "system" fn t_SetDpi(this: Obj, x: f32, y: f32) {
    with(this, |s| {
        if x == 0.0 && y == 0.0 {
            s.dpi = (96.0, 96.0);
        } else if x > 0.0 && y > 0.0 {
            s.dpi = (x, y);
        }
    });
}

extern "system" fn t_GetDpi(this: Obj, x: *mut f32, y: *mut f32) {
    let d = with(this, |s| s.dpi);
    put(x, d.0);
    put(y, d.1);
}

extern "system" fn t_GetSize(this: Obj, ret: *mut SizeF) -> *mut SizeF {
    let v = with(this, |s| SizeF {
        width: s.w as f32 * 96.0 / s.dpi.0,
        height: s.h as f32 * 96.0 / s.dpi.1,
    });
    put(ret, v);
    ret
}

extern "system" fn t_GetPixelSize(this: Obj, ret: *mut SizeU) -> *mut SizeU {
    put(ret, with(this, |s| SizeU { width: s.w, height: s.h }));
    ret
}

extern "system" fn t_GetMaximumBitmapSize(_this: Obj) -> u32 {
    16384
}

/// IsSupported: a software target of this one's pixel format (UNKNOWN
/// parts match anything).
extern "system" fn t_IsSupported(this: Obj, props: *const TargetProps) -> i32 {
    let Some(p) = read(props) else { return 0 };
    let f = with(this, |s| s.format);
    let format_ok = p.format.format == DXGI_FORMAT_UNKNOWN || p.format.format == f.format;
    let alpha_ok = p.format.alpha == ALPHA_UNKNOWN || p.format.alpha == f.alpha;
    i32::from(format_ok && alpha_ok && p.format.alpha != ALPHA_STRAIGHT)
}

// ---- ID2D1DCRenderTarget and ID2D1HwndRenderTarget ---------------------------------------

/// BindDC(hdc, rect): a DC (a window's, a memory DC or an enhanced
/// metafile's; not a metafile's) and the rectangle drawn into; a NULL or
/// invalid DC or a NULL rectangle is E_INVALIDARG and keeps the binding
/// there was (Wine's test_dc_target). The target's size becomes the
/// rectangle's.
extern "system" fn dc_BindDC(this: Obj, hdc: usize, rect: *const [i32; 4]) -> HResult {
    let Some(r) = read(rect) else { return E_INVALIDARG };
    // SAFETY: a plain GDI query.
    let kind = unsafe { GetObjectType(hdc) };
    if hdc == 0 || !matches!(kind, OBJ_DC | OBJ_MEMDC | OBJ_ENHMETADC) {
        return E_INVALIDARG;
    }
    if kind == OBJ_ENHMETADC {
        gap(b"ID2D1DCRenderTarget::BindDC(enhanced metafile DC)", 0);
    }
    let (w, h) = ((r[2] - r[0]).max(0) as u32, (r[3] - r[1]).max(0) as u32);
    with(this, |s| {
        let same = s.w == w && s.h == h;
        if let Backing::Dc { hdc: bound, origin, dib } = &mut s.backing {
            if !same || dib.is_none() {
                let Some(d) = Dib::new(w, h) else { return E_OUTOFMEMORY };
                *dib = Some(d);
            }
            *bound = hdc;
            *origin = (r[0], r[1]);
        }
        s.w = w;
        s.h = h;
        S_OK
    })
}

/// D2D1_WINDOW_STATE: OCCLUDED (1) while minimized.
extern "system" fn hwnd_CheckWindowState(this: Obj) -> u32 {
    let hwnd = hwnd_of(this);
    // SAFETY: a plain user32 query.
    u32::from(unsafe { IsIconic(hwnd) } != 0)
}

fn hwnd_of(this: Obj) -> usize {
    with(this, |s| if let Backing::Hwnd { hwnd, .. } = s.backing { hwnd } else { 0 })
}

/// Resize: a new back buffer of the size (its contents start black).
extern "system" fn hwnd_Resize(this: Obj, size: *const SizeU) -> HResult {
    let Some(size) = read(size) else { return E_INVALIDARG };
    let (w, h) = window_size(hwnd_of(this), size);
    let Some(d) = Dib::new(w, h) else { return E_OUTOFMEMORY };
    let old = with(this, |s| {
        s.w = w;
        s.h = h;
        match &mut s.backing {
            Backing::Hwnd { dib, .. } => dib.replace(d),
            _ => Some(d),
        }
    });
    drop(old);
    S_OK
}

extern "system" fn hwnd_GetHwnd(this: Obj) -> usize {
    hwnd_of(this)
}
