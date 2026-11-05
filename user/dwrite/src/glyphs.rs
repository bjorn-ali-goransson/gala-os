//! Glyphs as pixels and as outlines, what Skia's DirectWrite scaler asks
//! for to draw text (Chromium's browser and renderers):
//!
//! - IDWriteFactory::CreateGlyphRunAnalysis rasterizes a glyph run
//!   (fontdue over the face's file, as gdi32 draws Selawik) into an
//!   IDWriteGlyphRunAnalysis: GetAlphaTextureBounds and CreateAlphaTexture
//!   give the coverage as an aliased texture (1 byte per pixel, 0 or 255;
//!   DWRITE_RENDERING_MODE_ALIASED) or a ClearType 3x1 one (3 bytes per
//!   pixel, the same coverage in each: gala does not filter subpixels;
//!   the other modes). The texture of the other kind has empty bounds, as
//!   documented. OUTLINE mode cannot be analyzed (E_INVALIDARG).
//!   Transforms: scale and translation; rotation or skew is not
//!   supported yet (ledger, E_NOTIMPL).
//! - IDWriteFontFace::GetGlyphRunOutline gives the glyphs' TrueType
//!   outlines (ttf-parser) to an ID2D1SimplifiedGeometrySink in DIPs, y
//!   downward from the baseline: winding fill, a filled figure per
//!   contour, quadratic curves as cubic Béziers. The sink's owner closes
//!   it.

use crate::font::{face, font_of_face};
use crate::{E_INVALIDARG, E_NOTIMPL, E_POINTER, HResult, Iid, S_OK, guid, missing, query};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const E_NOT_SUFFICIENT_BUFFER: HResult = 0x8007_007a_u32 as i32;

const IID_ANALYSIS: Iid =
    guid(0x7d97dbf7, 0xe085, 0x42d4, [0x81, 0xe3, 0x6a, 0x88, 0x3b, 0xde, 0xd1, 0x18]);

/// DWRITE_GLYPH_RUN.
#[repr(C)]
pub struct GlyphRun {
    face: *const c_void,
    em: f32,
    count: u32,
    indices: *const u16,
    advances: *const f32,
    /// DWRITE_GLYPH_OFFSET: advanceOffset, ascenderOffset.
    offsets: *const [f32; 2],
    sideways: i32,
    bidi: u32,
}

/// The fontdue fonts of the system's faces, parsed on first use.
struct Fonts {
    lock: AtomicBool,
    fonts: core::cell::UnsafeCell<[Option<fontdue::Font>; 4]>,
}

// SAFETY: the cell is only touched under `lock`.
unsafe impl Sync for Fonts {}

static FONTS: Fonts = Fonts {
    lock: AtomicBool::new(false),
    fonts: core::cell::UnsafeCell::new([None, None, None, None]),
};

fn with_font<R>(i: usize, f: impl FnOnce(&fontdue::Font) -> R) -> Option<R> {
    let data = crate::font::data(i)?;
    while FONTS.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock; nothing below takes it again.
    let fonts = unsafe { &mut *FONTS.fonts.get() };
    let slot = fonts.get_mut(i);
    let r = slot.and_then(|slot| {
        if slot.is_none() {
            *slot = fontdue::Font::from_bytes(data, fontdue::FontSettings::default()).ok();
        }
        slot.as_ref().map(f)
    });
    FONTS.lock.store(false, Ordering::Release);
    r
}

// ---- IDWriteGlyphRunAnalysis ---------------------------------------------

#[repr(C)]
struct Analysis {
    vtbl: *const (),
    refs: AtomicU32,
    /// DWRITE_TEXTURE_ALIASED_1x1 (0) or _CLEARTYPE_3x1 (1).
    texture: u32,
    /// left, top, right, bottom in pixels (empty: nothing drawn).
    rect: [i32; 4],
    /// The coverage of `rect`, rows top first.
    coverage: Vec<u8>,
}

struct VTable<const N: usize>([*const (); N]);
// SAFETY: function pointers only.
unsafe impl<const N: usize> Sync for VTable<N> {}

static ANALYSIS_VTBL: VTable<6> = VTable([
    analysis_qi as *const (),
    analysis_add_ref as *const (),
    analysis_release as *const (),
    analysis_bounds as *const (),
    analysis_texture as *const (),
    analysis_blend_params as *const (),
]);

extern "system" fn analysis_qi(
    this: *const c_void,
    riid: *const Iid,
    out: *mut *const c_void,
) -> HResult {
    let r = query(this, &[IID_ANALYSIS], riid, out);
    if r == S_OK {
        analysis_add_ref(this);
    }
    r
}

extern "system" fn analysis_add_ref(this: *const c_void) -> u32 {
    // SAFETY: one of our Analysis objects.
    unsafe { (*this.cast::<Analysis>()).refs.fetch_add(1, Ordering::AcqRel) + 1 }
}

extern "system" fn analysis_release(this: *const c_void) -> u32 {
    // SAFETY: one of our Analysis objects, made by Box::into_raw.
    unsafe {
        let left = (*this.cast::<Analysis>()).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if left == 0 {
            drop(Box::from_raw(this.cast_mut().cast::<Analysis>()));
        }
        left
    }
}

/// GetAlphaTextureBounds(textureType, *bounds).
extern "system" fn analysis_bounds(
    this: *const c_void,
    texture: u32,
    out: *mut [i32; 4],
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    if texture > 1 {
        return E_INVALIDARG;
    }
    // SAFETY: one of our Analysis objects; the caller's output.
    unsafe {
        let a = &*this.cast::<Analysis>();
        *out = if texture == a.texture { a.rect } else { [0; 4] };
    }
    S_OK
}

/// CreateAlphaTexture(textureType, *bounds, *alpha, size): `bounds`'
/// pixels (any rectangle: outside the glyphs they are 0), 1 or 3 bytes
/// each.
extern "system" fn analysis_texture(
    this: *const c_void,
    texture: u32,
    bounds: *const [i32; 4],
    alpha: *mut u8,
    size: u32,
) -> HResult {
    if bounds.is_null() || alpha.is_null() || texture > 1 {
        return E_INVALIDARG;
    }
    // SAFETY: one of our Analysis objects; the caller's rectangle.
    let (a, [l, t, r, b]) = unsafe { (&*this.cast::<Analysis>(), *bounds) };
    let (w, h) = ((r - l).max(0) as usize, (b - t).max(0) as usize);
    let bytes = if texture == 0 { 1 } else { 3 };
    if (size as usize) < w * h * bytes {
        return E_NOT_SUFFICIENT_BUFFER;
    }
    // SAFETY: the caller's buffer of at least w * h * bytes.
    let out = unsafe { core::slice::from_raw_parts_mut(alpha, w * h * bytes) };
    out.fill(0);
    if texture != a.texture {
        return S_OK;
    }
    let [al, at, ar, ab] = a.rect;
    let aw = (ar - al) as usize;
    for y in t.max(at)..b.min(ab) {
        for x in l.max(al)..r.min(ar) {
            let v = a.coverage[(y - at) as usize * aw + (x - al) as usize];
            let o = ((y - t) as usize * w + (x - l) as usize) * bytes;
            out[o..o + bytes].fill(v);
        }
    }
    S_OK
}

/// GetAlphaBlendParams(renderingParams, *gamma, *enhancedContrast,
/// *clearTypeLevel): linear coverage, no contrast, no ClearType.
extern "system" fn analysis_blend_params(
    _this: *const c_void,
    _params: *const c_void,
    gamma: *mut f32,
    contrast: *mut f32,
    level: *mut f32,
) -> HResult {
    if gamma.is_null() || contrast.is_null() || level.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's outputs.
    unsafe {
        *gamma = 1.0;
        *contrast = 0.0;
        *level = 0.0;
    }
    S_OK
}

/// A glyph's coverage placed on the pixel grid.
struct Placed {
    left: i32,
    top: i32,
    width: usize,
    height: usize,
    coverage: Vec<u8>,
}

/// IDWriteFactory::CreateGlyphRunAnalysis(glyphRun, pixelsPerDip,
/// transform, renderingMode, measuringMode, baselineOriginX,
/// baselineOriginY, **analysis).
#[allow(clippy::too_many_arguments)]
pub extern "system" fn create_glyph_run_analysis(
    _this: *const c_void,
    run: *const GlyphRun,
    pixels_per_dip: f32,
    transform: *const [f32; 6],
    mode: u32,
    _measuring: u32,
    x: f32,
    y: f32,
    out: *mut *const c_void,
) -> HResult {
    if out.is_null() || run.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's output.
    unsafe { *out = core::ptr::null() };
    // DWRITE_RENDERING_MODE_OUTLINE (6) and unknown modes.
    if mode >= 6 {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's DWRITE_GLYPH_RUN.
    let run = unsafe { &*run };
    let Some(font) = font_of_face(run.face) else {
        missing(b"dwrite.dll!CreateGlyphRunAnalysis(not a gala face)", 0);
        return E_INVALIDARG;
    };
    if (run.indices.is_null() && run.count != 0) || run.sideways != 0 {
        if run.sideways != 0 {
            missing(b"dwrite.dll!CreateGlyphRunAnalysis(sideways)", 0);
            return E_NOTIMPL;
        }
        return E_INVALIDARG;
    }
    // [m11, m12, m21, m22, dx, dy]: scale and translation only.
    let [m11, m12, m21, m22, dx, dy] = if transform.is_null() {
        [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
    } else {
        // SAFETY: the caller's DWRITE_MATRIX.
        unsafe { *transform }
    };
    if m12 != 0.0 || m21 != 0.0 || m11 <= 0.0 || m22 <= 0.0 {
        missing(b"dwrite.dll!CreateGlyphRunAnalysis(rotation or skew)", 0);
        return E_NOTIMPL;
    }
    let ppd = if pixels_per_dip > 0.0 { pixels_per_dip } else { 1.0 };
    // Pixels: x' = (x * m11 + dx) * ppd, the same for y with m22 and dy;
    // the glyphs are rasterized at the vertical scale (fontdue scales
    // uniformly).
    let px = run.em * m22 * ppd;
    let rtl = run.bidi & 1 == 1;
    let mut placed: Vec<Placed> = Vec::new();
    let made = with_font(font, |f| {
        let mut pen = 0.0f32;
        for i in 0..run.count as usize {
            // SAFETY: the caller's `count` indices, advances and offsets.
            let (index, advance, offset) = unsafe {
                let index = *run.indices.add(i);
                let advance = if run.advances.is_null() {
                    f.metrics_indexed(index, run.em).advance_width
                } else {
                    *run.advances.add(i)
                };
                let offset = if run.offsets.is_null() { [0.0, 0.0] } else { *run.offsets.add(i) };
                (index, advance, offset)
            };
            if rtl {
                pen -= advance;
            }
            let gx = x + pen + if rtl { -offset[0] } else { offset[0] };
            let gy = y - offset[1];
            if !rtl {
                pen += advance;
            }
            let (m, coverage) = f.rasterize_indexed(index, px);
            if m.width == 0 || m.height == 0 {
                continue;
            }
            let ox = libm_round((gx * m11 + dx) * ppd);
            let oy = libm_round((gy * m22 + dy) * ppd);
            placed.push(Placed {
                left: ox + m.xmin,
                top: oy - m.ymin - m.height as i32,
                width: m.width,
                height: m.height,
                coverage,
            });
        }
    });
    if made.is_none() {
        missing(b"dwrite.dll!CreateGlyphRunAnalysis(font file)", font as u64);
        return crate::E_FAIL;
    }
    let mut rect = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
    for p in &placed {
        rect[0] = rect[0].min(p.left);
        rect[1] = rect[1].min(p.top);
        rect[2] = rect[2].max(p.left + p.width as i32);
        rect[3] = rect[3].max(p.top + p.height as i32);
    }
    if placed.is_empty() {
        rect = [0; 4];
    }
    let (w, h) = ((rect[2] - rect[0]) as usize, (rect[3] - rect[1]) as usize);
    let mut coverage = alloc::vec![0u8; w * h];
    let aliased = mode == 1;
    for p in &placed {
        for row in 0..p.height {
            for col in 0..p.width {
                let v = p.coverage[row * p.width + col];
                let at = (p.top - rect[1]) as usize + row;
                let o = at * w + (p.left - rect[0]) as usize + col;
                let v = if aliased { if v >= 128 { 255 } else { 0 } } else { v };
                coverage[o] = coverage[o].max(v);
            }
        }
    }
    let analysis = Box::new(Analysis {
        vtbl: ANALYSIS_VTBL.0.as_ptr().cast(),
        refs: AtomicU32::new(1),
        texture: if aliased { 0 } else { 1 },
        rect,
        coverage,
    });
    // SAFETY: the caller's output.
    unsafe { *out = Box::into_raw(analysis).cast() };
    S_OK
}

/// Rounds to the nearest integer (halves away from zero).
fn libm_round(v: f32) -> i32 {
    if v >= 0.0 { (v + 0.5) as i32 } else { (v - 0.5) as i32 }
}

// ---- GetGlyphRunOutline ------------------------------------------------

/// D2D1_POINT_2F.
#[repr(C)]
#[derive(Clone, Copy)]
struct Point {
    x: f32,
    y: f32,
}

type Method<F> = F;

/// The sink's methods (ID2D1SimplifiedGeometrySink).
struct Sink {
    this: *const c_void,
    begin: Method<extern "system" fn(*const c_void, Point, u32)>,
    lines: Method<extern "system" fn(*const c_void, *const Point, u32)>,
    beziers: Method<extern "system" fn(*const c_void, *const [Point; 3], u32)>,
    end: Method<extern "system" fn(*const c_void, u32)>,
    /// The glyph's origin and scale (font units to DIPs, y flipped).
    origin: (f32, f32),
    scale: f32,
    open: bool,
    last: Point,
}

impl Sink {
    fn at(&self, x: f32, y: f32) -> Point {
        Point { x: self.origin.0 + x * self.scale, y: self.origin.1 - y * self.scale }
    }
    fn end_figure(&mut self) {
        if self.open {
            // D2D1_FIGURE_END_CLOSED.
            (self.end)(self.this, 1);
            self.open = false;
        }
    }
}

impl ttf_parser::OutlineBuilder for Sink {
    fn move_to(&mut self, x: f32, y: f32) {
        self.end_figure();
        let p = self.at(x, y);
        // D2D1_FIGURE_BEGIN_FILLED.
        (self.begin)(self.this, p, 0);
        self.open = true;
        self.last = p;
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        (self.lines)(self.this, &p, 1);
        self.last = p;
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (q, p) = (self.at(x1, y1), self.at(x, y));
        let s = self.last;
        // The quadratic as a cubic: control points 2/3 of the way to q.
        let c1 = Point { x: s.x + 2.0 / 3.0 * (q.x - s.x), y: s.y + 2.0 / 3.0 * (q.y - s.y) };
        let c2 = Point { x: p.x + 2.0 / 3.0 * (q.x - p.x), y: p.y + 2.0 / 3.0 * (q.y - p.y) };
        (self.beziers)(self.this, &[c1, c2, p], 1);
        self.last = p;
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let seg = [self.at(x1, y1), self.at(x2, y2), self.at(x, y)];
        (self.beziers)(self.this, &seg, 1);
        self.last = seg[2];
    }
    fn close(&mut self) {
        self.end_figure();
    }
}

/// Method `slot` of the caller's COM object.
///
/// # Safety
/// `this` is a COM object whose vtable has that slot with type `F`.
unsafe fn method<F: Copy>(this: *const c_void, slot: usize) -> F {
    // SAFETY: as the caller promises.
    unsafe {
        let vtbl = *this.cast::<*const usize>();
        core::mem::transmute_copy(&*vtbl.add(slot))
    }
}

/// IDWriteFontFace::GetGlyphRunOutline for font `font`.
#[allow(clippy::too_many_arguments)]
pub fn outline(
    font: usize,
    em: f32,
    glyphs: *const u16,
    advances: *const f32,
    offsets: *const [f32; 2],
    count: u32,
    sideways: bool,
    rtl: bool,
    sink: *const c_void,
) -> HResult {
    if sink.is_null() || (glyphs.is_null() && count != 0) {
        return E_INVALIDARG;
    }
    if sideways {
        missing(b"dwrite.dll!GetGlyphRunOutline(sideways)", 0);
        return E_NOTIMPL;
    }
    let Some(f) = face(font) else { return crate::E_FAIL };
    let scale = em / f32::from(f.units_per_em().max(1));
    // SAFETY: the caller's ID2D1SimplifiedGeometrySink (slots from
    // d2d1.h: SetFillMode 3, BeginFigure 5, AddLines 6, AddBeziers 7,
    // EndFigure 8).
    let mut s = unsafe {
        let fill: extern "system" fn(*const c_void, u32) = method(sink, 3);
        // D2D1_FILL_MODE_WINDING.
        fill(sink, 1);
        Sink {
            this: sink,
            begin: method(sink, 5),
            lines: method(sink, 6),
            beziers: method(sink, 7),
            end: method(sink, 8),
            origin: (0.0, 0.0),
            scale,
            open: false,
            last: Point { x: 0.0, y: 0.0 },
        }
    };
    let mut pen = 0.0f32;
    for i in 0..count as usize {
        // SAFETY: the caller's `count` glyphs, advances and offsets.
        let (index, advance, offset) = unsafe {
            let index = *glyphs.add(i);
            let advance = if advances.is_null() {
                f32::from(f.glyph_hor_advance(ttf_parser::GlyphId(index)).unwrap_or(0)) * scale
            } else {
                *advances.add(i)
            };
            let offset = if offsets.is_null() { [0.0, 0.0] } else { *offsets.add(i) };
            (index, advance, offset)
        };
        if rtl {
            pen -= advance;
        }
        s.origin = (pen + if rtl { -offset[0] } else { offset[0] }, -offset[1]);
        if !rtl {
            pen += advance;
        }
        let _ = f.outline_glyph(ttf_parser::GlyphId(index), &mut s);
        s.end_figure();
    }
    S_OK
}
