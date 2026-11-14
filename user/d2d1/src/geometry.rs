//! Geometries: rectangles, rounded rectangles, ellipses and paths
//! (ID2D1PathGeometry, filled through its ID2D1GeometrySink), all kept as
//! `Path`s of figures of lines and cubic Béziers in their own
//! coordinates; the render target flattens them through its transform
//! (`raster`).
//!
//! A path geometry is first empty (its counts and bounds
//! D2DERR_WRONG_STATE), then open while its sink takes figures, then
//! closed by the sink's Close; a call out of that order (a second Open, a
//! segment outside a figure, BeginFigure inside one, Close with a figure
//! open) puts it in error, and Close and the queries answer
//! D2DERR_WRONG_STATE from then on (Wine's test_path_geometry). A closed
//! figure counts one segment more than it was given: the closing line.

use alloc::vec::Vec;

use crate::raster::{self, Poly};
use crate::{
    D2DERR_WRONG_STATE, E_INVALIDARG, E_OUTOFMEMORY, Guid, HResult, IID_ELLIPSE_GEOMETRY,
    IID_GEOMETRY, IID_GEOMETRY_SINK, IID_PATH_GEOMETRY, IID_RECTANGLE_GEOMETRY, IID_RESOURCE,
    IID_ROUNDED_RECTANGLE_GEOMETRY, IID_SIMPLIFIED_SINK, Matrix, Obj, Point, RectF, Res, S_OK,
    Spin, VTable, add_ref, get_factory, give, not_implemented, pt, push, put, query, read,
    release_object, vtable_of,
};

/// A segment of a figure, ending at its last point.
#[derive(Clone, Copy)]
pub enum Seg {
    Line(Point),
    Cubic(Point, Point, Point),
}

pub struct Fig {
    pub start: Point,
    pub segs: Vec<Seg>,
    pub closed: bool,
    pub filled: bool,
}

/// Figures and their fill rule.
pub struct Path {
    pub figs: Vec<Fig>,
    pub winding: bool,
}

/// The circle's control-point distance for a quarter arc.
const KAPPA: f32 = 0.552_284_8;

fn seg_vec<const N: usize>(s: [Seg; N]) -> Option<Vec<Seg>> {
    let mut v = Vec::new();
    v.try_reserve_exact(N).ok()?;
    v.extend_from_slice(&s);
    Some(v)
}

fn one_fig(f: Fig) -> Option<Path> {
    let mut figs = Vec::new();
    if !push(&mut figs, f) {
        return None;
    }
    Some(Path { figs, winding: false })
}

/// A rectangle, clockwise from its top-left corner.
pub fn rect_path(r: &RectF) -> Option<Path> {
    let segs = seg_vec([
        Seg::Line(pt(r.right, r.top)),
        Seg::Line(pt(r.right, r.bottom)),
        Seg::Line(pt(r.left, r.bottom)),
    ])?;
    one_fig(Fig { start: pt(r.left, r.top), segs, closed: true, filled: true })
}

/// An ellipse as four quarter Béziers.
pub fn ellipse_path(c: Point, rx: f32, ry: f32) -> Option<Path> {
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    let segs = seg_vec([
        Seg::Cubic(pt(c.x + kx, c.y - ry), pt(c.x + rx, c.y - ky), pt(c.x + rx, c.y)),
        Seg::Cubic(pt(c.x + rx, c.y + ky), pt(c.x + kx, c.y + ry), pt(c.x, c.y + ry)),
        Seg::Cubic(pt(c.x - kx, c.y + ry), pt(c.x - rx, c.y + ky), pt(c.x - rx, c.y)),
        Seg::Cubic(pt(c.x - rx, c.y - ky), pt(c.x - kx, c.y - ry), pt(c.x, c.y - ry)),
    ])?;
    one_fig(Fig { start: pt(c.x, c.y - ry), segs, closed: true, filled: true })
}

/// A rounded rectangle (radii at most half the sides).
pub fn rounded_path(r: &RectF, rx: f32, ry: f32) -> Option<Path> {
    let w = (r.right - r.left).abs();
    let h = (r.bottom - r.top).abs();
    let rx = rx.abs().min(w / 2.0);
    let ry = ry.abs().min(h / 2.0);
    if rx <= 0.0 || ry <= 0.0 {
        return rect_path(r);
    }
    let (l, t, rt, b) = (r.left, r.top, r.right, r.bottom);
    let (kx, ky) = (rx * (1.0 - KAPPA), ry * (1.0 - KAPPA));
    let segs = seg_vec([
        Seg::Line(pt(rt - rx, t)),
        Seg::Cubic(pt(rt - kx, t), pt(rt, t + ky), pt(rt, t + ry)),
        Seg::Line(pt(rt, b - ry)),
        Seg::Cubic(pt(rt, b - ky), pt(rt - kx, b), pt(rt - rx, b)),
        Seg::Line(pt(l + rx, b)),
        Seg::Cubic(pt(l + kx, b), pt(l, b - ky), pt(l, b - ry)),
        Seg::Line(pt(l, t + ry)),
        Seg::Cubic(pt(l, t + ky), pt(l + kx, t), pt(l + rx, t)),
    ])?;
    one_fig(Fig { start: pt(l + rx, t), segs, closed: true, filled: true })
}

/// A two-point open figure (DrawLine).
pub fn line_path(a: Point, b: Point) -> Option<Path> {
    one_fig(Fig { start: a, segs: seg_vec([Seg::Line(b)])?, closed: false, filled: false })
}

impl Path {
    /// The figures flattened through `m`.
    pub fn flatten(&self, m: &Matrix) -> Option<Vec<Poly>> {
        let mut out = Vec::new();
        for f in &self.figs {
            let mut pts = Vec::new();
            let mut cur = m.apply(f.start);
            if !push(&mut pts, cur) {
                return None;
            }
            for s in &f.segs {
                match *s {
                    Seg::Line(p) => {
                        cur = m.apply(p);
                        if !push(&mut pts, cur) {
                            return None;
                        }
                    }
                    Seg::Cubic(a, b, c) => {
                        let (a, b, c) = (m.apply(a), m.apply(b), m.apply(c));
                        if !raster::flatten_cubic(&mut pts, cur, a, b, c) {
                            return None;
                        }
                        cur = c;
                    }
                }
            }
            if !push(&mut out, Poly { pts, closed: f.closed, filled: f.filled }) {
                return None;
            }
        }
        Some(out)
    }

    /// The filled figures' polygons through `m`.
    pub fn fill_polys(&self, m: &Matrix) -> Option<raster::Polys> {
        let mut out = Vec::new();
        for p in self.flatten(m)? {
            if p.filled && p.pts.len() > 2 && !push(&mut out, p.pts) {
                return None;
            }
        }
        Some(out)
    }

    fn segment_count(&self) -> u32 {
        self.figs.iter().map(|f| f.segs.len() as u32 + u32::from(f.closed)).sum()
    }

    /// The bounds of every point through `m` (curves as flattened); an
    /// empty path's are inverted infinities.
    fn bounds(&self, m: &Matrix) -> Option<RectF> {
        let mut r = RectF {
            left: f32::INFINITY,
            top: f32::INFINITY,
            right: f32::NEG_INFINITY,
            bottom: f32::NEG_INFINITY,
        };
        for p in self.flatten(m)? {
            for q in p.pts {
                r.left = r.left.min(q.x);
                r.top = r.top.min(q.y);
                r.right = r.right.max(q.x);
                r.bottom = r.bottom.max(q.y);
            }
        }
        Some(r)
    }

    /// Whether `p` is inside the filled figures through `m`.
    fn contains(&self, p: Point, m: &Matrix) -> Option<bool> {
        Some(contains_point(&self.fill_polys(m)?, p, self.winding))
    }
}

/// The fill rule at point `p` over `polys`.
pub fn contains_point(polys: &[Vec<Point>], p: Point, winding: bool) -> bool {
    let mut w = 0;
    for poly in polys {
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            if (a.y <= p.y) != (b.y <= p.y) {
                let x = a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y);
                if x > p.x {
                    w += if b.y > a.y { 1 } else { -1 };
                }
            }
        }
    }
    if winding { w != 0 } else { w & 1 != 0 }
}

// ---- The geometry objects -------------------------------------------------

/// What a geometry object is.
pub enum Shape {
    Rect(RectF),
    Rounded(RectF, f32, f32),
    Ellipse(Point, f32, f32),
    Path(Spin<PathState>),
}

#[derive(Clone, Copy, PartialEq)]
pub enum PathPhase {
    Empty,
    Open,
    Closed,
    Error,
}

pub struct PathState {
    pub phase: PathPhase,
    pub path: Path,
    /// A figure begun and not yet ended (in the sink).
    pub in_figure: bool,
    pub bad_flags: bool,
}

#[repr(C)]
pub struct Geometry {
    res: Res,
    pub shape: Shape,
}

/// The ID2D1Geometry methods every shape shares (slots 3 to 16).
macro_rules! geometry_slots {
    () => {
        [
            geo_query as *const (),
            add_ref as *const (),
            geo_release as *const (),
            get_factory as *const (),
            geo_GetBounds as *const (),
            geo_GetWidenedBounds as *const (),
            geo_StrokeContainsPoint as *const (),
            geo_FillContainsPoint as *const (),
            geo_CompareWithGeometry as *const (),
            geo_Simplify as *const (),
            geo_Tessellate as *const (),
            geo_CombineWithGeometry as *const (),
            geo_Outline as *const (),
            geo_ComputeArea as *const (),
            geo_ComputeLength as *const (),
            geo_ComputePointAtLength as *const (),
            geo_Widen as *const (),
        ]
    };
}

const fn with_slots<const N: usize>(extra: [*const (); N]) -> [*const (); 21] {
    let base = geometry_slots!();
    let mut out = [core::ptr::null::<()>(); 21];
    let mut i = 0;
    while i < 17 {
        out[i] = base[i];
        i += 1;
    }
    let mut j = 0;
    while j < N {
        out[17 + j] = extra[j];
        j += 1;
    }
    out
}

static RECT_VTBL: VTable<21> = VTable(with_slots([rect_GetRect as *const ()]));
static ROUNDED_VTBL: VTable<21> = VTable(with_slots([rounded_GetRoundedRect as *const ()]));
static ELLIPSE_VTBL: VTable<21> = VTable(with_slots([ellipse_GetEllipse as *const ()]));
static PATH_VTBL: VTable<21> = VTable(with_slots([
    path_Open as *const (),
    path_Stream as *const (),
    path_GetSegmentCount as *const (),
    path_GetFigureCount as *const (),
]));

/// Whether `o` is one of this DLL's geometries.
pub fn is_geometry(o: Obj) -> bool {
    let v = vtable_of(o);
    [RECT_VTBL.addr(), ROUNDED_VTBL.addr(), ELLIPSE_VTBL.addr(), PATH_VTBL.addr()].contains(&v)
}

fn geo<'a>(this: Obj) -> &'a Geometry {
    // SAFETY: one of our geometries.
    unsafe { &*(this as *const Geometry) }
}

/// A new geometry for the factory's Create* method.
pub fn create(factory: Obj, shape: Shape, out: *mut Obj) -> HResult {
    if out.is_null() {
        return crate::E_POINTER;
    }
    let vtbl = match shape {
        Shape::Rect(_) => RECT_VTBL.addr(),
        Shape::Rounded(..) => ROUNDED_VTBL.addr(),
        Shape::Ellipse(..) => ELLIPSE_VTBL.addr(),
        Shape::Path(_) => PATH_VTBL.addr(),
    };
    give(out, Geometry { res: Res::new(vtbl, factory), shape })
}

pub fn new_path_shape() -> Shape {
    Shape::Path(Spin::new(PathState {
        phase: PathPhase::Empty,
        path: Path { figs: Vec::new(), winding: false },
        in_figure: false,
        bad_flags: false,
    }))
}

/// Runs `f` on the geometry's path: a shape's own (built on the spot),
/// a path geometry's when it is closed (else D2DERR_WRONG_STATE).
pub fn with_path<R>(o: Obj, f: impl FnOnce(&Path) -> R) -> Result<R, HResult> {
    let g = geo(o);
    let built = match &g.shape {
        Shape::Rect(r) => rect_path(r),
        Shape::Rounded(r, rx, ry) => rounded_path(r, *rx, *ry),
        Shape::Ellipse(c, rx, ry) => ellipse_path(*c, *rx, *ry),
        Shape::Path(s) => {
            return s.with(|st| {
                if st.phase == PathPhase::Closed {
                    Ok(f(&st.path))
                } else {
                    Err(D2DERR_WRONG_STATE)
                }
            });
        }
    };
    built.map(|p| f(&p)).ok_or(E_OUTOFMEMORY)
}

extern "system" fn geo_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    let own = match &geo(this).shape {
        Shape::Rect(_) => IID_RECTANGLE_GEOMETRY,
        Shape::Rounded(..) => IID_ROUNDED_RECTANGLE_GEOMETRY,
        Shape::Ellipse(..) => IID_ELLIPSE_GEOMETRY,
        Shape::Path(_) => IID_PATH_GEOMETRY,
    };
    query(this, b"ID2D1Geometry", &[IID_RESOURCE, IID_GEOMETRY, own], riid, out)
}

extern "system" fn geo_release(this: Obj) -> u32 {
    release_object::<Geometry>(this)
}

fn matrix_or_identity(m: *const Matrix) -> Matrix {
    read(m).unwrap_or(Matrix::IDENTITY)
}

extern "system" fn geo_GetBounds(this: Obj, m: *const Matrix, out: *mut RectF) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let m = matrix_or_identity(m);
    match with_path(this, |p| p.bounds(&m)) {
        Ok(Some(r)) => {
            put(out, r);
            S_OK
        }
        Ok(None) => E_OUTOFMEMORY,
        Err(e) => e,
    }
}

extern "system" fn geo_GetWidenedBounds(
    this: Obj,
    width: f32,
    _style: Obj,
    m: *const Matrix,
    _tolerance: f32,
    out: *mut RectF,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let m = matrix_or_identity(m);
    let h = width.abs() / 2.0 * m.max_scale();
    match with_path(this, |p| p.bounds(&m)) {
        Ok(Some(r)) => {
            put(
                out,
                RectF {
                    left: r.left - h,
                    top: r.top - h,
                    right: r.right + h,
                    bottom: r.bottom + h,
                },
            );
            S_OK
        }
        Ok(None) => E_OUTOFMEMORY,
        Err(e) => e,
    }
}

extern "system" fn geo_StrokeContainsPoint(
    this: Obj,
    p: Point,
    width: f32,
    style: Obj,
    m: *const Matrix,
    _tolerance: f32,
    out: *mut i32,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let m = matrix_or_identity(m);
    let st = crate::factory::stroke_style_of(style);
    let r = with_path(this, |path| {
        let figs = path.flatten(&m)?;
        let polys = raster::stroke(&figs, width * m.max_scale(), &st)?;
        Some(contains_point(&polys, p, true))
    });
    match r {
        Ok(Some(c)) => {
            put(out, i32::from(c));
            S_OK
        }
        Ok(None) => E_OUTOFMEMORY,
        Err(e) => e,
    }
}

extern "system" fn geo_FillContainsPoint(
    this: Obj,
    p: Point,
    m: *const Matrix,
    _tolerance: f32,
    out: *mut i32,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let m = matrix_or_identity(m);
    match with_path(this, |path| path.contains(p, &m)) {
        Ok(Some(c)) => {
            put(out, i32::from(c));
            S_OK
        }
        Ok(None) => E_OUTOFMEMORY,
        Err(e) => e,
    }
}

extern "system" fn geo_CompareWithGeometry(
    _this: Obj,
    _other: Obj,
    _m: *const Matrix,
    _tolerance: f32,
    _out: *mut u32,
) -> HResult {
    not_implemented(b"ID2D1Geometry::CompareWithGeometry")
}

extern "system" fn geo_Simplify(
    _this: Obj,
    _option: u32,
    _m: *const Matrix,
    _tolerance: f32,
    _sink: Obj,
) -> HResult {
    not_implemented(b"ID2D1Geometry::Simplify")
}

extern "system" fn geo_Tessellate(
    _this: Obj,
    _m: *const Matrix,
    _tolerance: f32,
    _sink: Obj,
) -> HResult {
    not_implemented(b"ID2D1Geometry::Tessellate")
}

extern "system" fn geo_CombineWithGeometry(
    _this: Obj,
    _other: Obj,
    _mode: u32,
    _m: *const Matrix,
    _tolerance: f32,
    _sink: Obj,
) -> HResult {
    not_implemented(b"ID2D1Geometry::CombineWithGeometry")
}

extern "system" fn geo_Outline(
    _this: Obj,
    _m: *const Matrix,
    _tolerance: f32,
    _sink: Obj,
) -> HResult {
    not_implemented(b"ID2D1Geometry::Outline")
}

/// ComputeArea: the filled figures' areas (as flattened; overlapping
/// figures are counted once each).
extern "system" fn geo_ComputeArea(
    this: Obj,
    m: *const Matrix,
    _tolerance: f32,
    out: *mut f32,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let m = matrix_or_identity(m);
    let r = with_path(this, |path| {
        let polys = path.fill_polys(&m)?;
        let mut total = 0.0;
        for p in &polys {
            let mut a = 0.0;
            for i in 0..p.len() {
                let (u, v) = (p[i], p[(i + 1) % p.len()]);
                a += u.x * v.y - v.x * u.y;
            }
            total += (a / 2.0).abs();
        }
        Some(total)
    });
    match r {
        Ok(Some(a)) => {
            put(out, a);
            S_OK
        }
        Ok(None) => E_OUTOFMEMORY,
        Err(e) => e,
    }
}

/// ComputeLength: the figures' lengths, closing lines included.
extern "system" fn geo_ComputeLength(
    this: Obj,
    m: *const Matrix,
    _tolerance: f32,
    out: *mut f32,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let m = matrix_or_identity(m);
    let r = with_path(this, |path| {
        let figs = path.flatten(&m)?;
        let mut total = 0.0;
        for f in &figs {
            let n = f.pts.len();
            let segs = if f.closed { n } else { n.saturating_sub(1) };
            for i in 0..segs {
                let (a, b) = (f.pts[i], f.pts[(i + 1) % n]);
                total += libm::sqrtf((b.x - a.x) * (b.x - a.x) + (b.y - a.y) * (b.y - a.y));
            }
        }
        Some(total)
    });
    match r {
        Ok(Some(l)) => {
            put(out, l);
            S_OK
        }
        Ok(None) => E_OUTOFMEMORY,
        Err(e) => e,
    }
}

extern "system" fn geo_ComputePointAtLength(
    _this: Obj,
    _length: f32,
    _m: *const Matrix,
    _tolerance: f32,
    _point: *mut Point,
    _tangent: *mut Point,
) -> HResult {
    not_implemented(b"ID2D1Geometry::ComputePointAtLength")
}

extern "system" fn geo_Widen(
    _this: Obj,
    _width: f32,
    _style: Obj,
    _m: *const Matrix,
    _tolerance: f32,
    _sink: Obj,
) -> HResult {
    not_implemented(b"ID2D1Geometry::Widen")
}

extern "system" fn rect_GetRect(this: Obj, out: *mut RectF) {
    if let Shape::Rect(r) = &geo(this).shape {
        put(out, *r);
    }
}

/// D2D1_ROUNDED_RECT.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RoundedRect {
    pub rect: RectF,
    pub rx: f32,
    pub ry: f32,
}

/// D2D1_ELLIPSE.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ellipse {
    pub center: Point,
    pub rx: f32,
    pub ry: f32,
}

extern "system" fn rounded_GetRoundedRect(this: Obj, out: *mut RoundedRect) {
    if let Shape::Rounded(rect, rx, ry) = &geo(this).shape {
        put(out, RoundedRect { rect: *rect, rx: *rx, ry: *ry });
    }
}

extern "system" fn ellipse_GetEllipse(this: Obj, out: *mut Ellipse) {
    if let Shape::Ellipse(center, rx, ry) = &geo(this).shape {
        put(out, Ellipse { center: *center, rx: *rx, ry: *ry });
    }
}

fn path_state<R>(this: Obj, f: impl FnOnce(&mut PathState) -> R) -> Option<R> {
    match &geo(this).shape {
        Shape::Path(s) => Some(s.with(f)),
        _ => None,
    }
}

/// Open: the sink, once (a second Open, or one after Close,
/// D2DERR_WRONG_STATE).
extern "system" fn path_Open(this: Obj, out: *mut Obj) -> HResult {
    if out.is_null() {
        return crate::E_POINTER;
    }
    let ok = path_state(this, |s| {
        if s.phase == PathPhase::Empty {
            s.phase = PathPhase::Open;
            true
        } else {
            false
        }
    });
    if ok != Some(true) {
        put(out, core::ptr::null_mut());
        return D2DERR_WRONG_STATE;
    }
    add_ref(this);
    let r = give(out, Sink { hdr: crate::Header::new(SINK_VTBL.addr()), path: this });
    if r != S_OK {
        release_object::<Geometry>(this);
    }
    r
}

extern "system" fn path_Stream(_this: Obj, _sink: Obj) -> HResult {
    not_implemented(b"ID2D1PathGeometry::Stream")
}

fn closed_count(this: Obj, out: *mut u32, f: impl FnOnce(&Path) -> u32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let r =
        path_state(this, |s| if s.phase == PathPhase::Closed { Some(f(&s.path)) } else { None });
    match r.flatten() {
        Some(n) => {
            put(out, n);
            S_OK
        }
        None => D2DERR_WRONG_STATE,
    }
}

extern "system" fn path_GetSegmentCount(this: Obj, out: *mut u32) -> HResult {
    closed_count(this, out, |p| p.segment_count())
}

extern "system" fn path_GetFigureCount(this: Obj, out: *mut u32) -> HResult {
    closed_count(this, out, |p| p.figs.len() as u32)
}

// ---- The sink ---------------------------------------------------------------

#[repr(C)]
struct Sink {
    hdr: crate::Header,
    /// The path geometry it fills (a reference held).
    path: Obj,
}

impl Drop for Sink {
    fn drop(&mut self) {
        release_object::<Geometry>(self.path);
    }
}

static SINK_VTBL: VTable<15> = VTable([
    sink_query as *const (),
    add_ref as *const (),
    sink_release as *const (),
    sink_SetFillMode as *const (),
    sink_SetSegmentFlags as *const (),
    sink_BeginFigure as *const (),
    sink_AddLines as *const (),
    sink_AddBeziers as *const (),
    sink_EndFigure as *const (),
    sink_Close as *const (),
    sink_AddLine as *const (),
    sink_AddBezier as *const (),
    sink_AddQuadraticBezier as *const (),
    sink_AddQuadraticBeziers as *const (),
    sink_AddArc as *const (),
]);

extern "system" fn sink_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    query(this, b"ID2D1GeometrySink", &[IID_SIMPLIFIED_SINK, IID_GEOMETRY_SINK], riid, out)
}

extern "system" fn sink_release(this: Obj) -> u32 {
    release_object::<Sink>(this)
}

/// Runs `f` on the sink's path while it is open (nothing once closed or
/// in error).
fn sink_with(this: Obj, f: impl FnOnce(&mut PathState)) {
    // SAFETY: one of our sinks.
    let path = unsafe { (*(this as *const Sink)).path };
    path_state(path, |s| {
        if s.phase == PathPhase::Open {
            f(s);
        }
    });
}

/// A segment: only inside a figure (else the path is in error).
fn add_segs(this: Obj, segs: impl Iterator<Item = Seg>) {
    sink_with(this, |s| {
        if !s.in_figure {
            s.phase = PathPhase::Error;
            return;
        }
        let Some(f) = s.path.figs.last_mut() else { return };
        for g in segs {
            if !push(&mut f.segs, g) {
                s.phase = PathPhase::Error;
                return;
            }
        }
    });
}

extern "system" fn sink_SetFillMode(this: Obj, mode: u32) {
    sink_with(this, |s| s.path.winding = mode == 1);
}

extern "system" fn sink_SetSegmentFlags(this: Obj, flags: u32) {
    sink_with(this, |s| {
        // D2D1_PATH_SEGMENT_FORCE_UNSTROKED | FORCE_ROUND_LINE_JOIN.
        if flags & !3 != 0 {
            s.bad_flags = true;
        } else if flags != 0 {
            crate::gap(b"ID2D1GeometrySink::SetSegmentFlags", flags.into());
        }
    });
}

extern "system" fn sink_BeginFigure(this: Obj, start: Point, begin: u32) {
    sink_with(this, |s| {
        if s.in_figure {
            s.phase = PathPhase::Error;
            return;
        }
        // D2D1_FIGURE_BEGIN_FILLED 0, HOLLOW 1.
        let f = Fig { start, segs: Vec::new(), closed: false, filled: begin == 0 };
        if push(&mut s.path.figs, f) {
            s.in_figure = true;
        } else {
            s.phase = PathPhase::Error;
        }
    });
}

extern "system" fn sink_EndFigure(this: Obj, end: u32) {
    sink_with(this, |s| {
        if !s.in_figure {
            s.phase = PathPhase::Error;
            return;
        }
        if let Some(f) = s.path.figs.last_mut() {
            // D2D1_FIGURE_END_OPEN 0, CLOSED 1.
            f.closed = end == 1;
        }
        s.in_figure = false;
    });
}

extern "system" fn sink_Close(this: Obj) -> HResult {
    // SAFETY: one of our sinks.
    let path = unsafe { (*(this as *const Sink)).path };
    path_state(path, |s| match s.phase {
        PathPhase::Open if s.bad_flags => {
            s.phase = PathPhase::Error;
            E_INVALIDARG
        }
        PathPhase::Open if !s.in_figure => {
            s.phase = PathPhase::Closed;
            S_OK
        }
        PathPhase::Open => {
            s.phase = PathPhase::Error;
            D2DERR_WRONG_STATE
        }
        _ => D2DERR_WRONG_STATE,
    })
    .unwrap_or(D2DERR_WRONG_STATE)
}

/// Reads `n` items the caller passed (an empty slice for NULL).
fn items<'a, T>(p: *const T, n: u32) -> &'a [T] {
    if p.is_null() || n == 0 {
        &[]
    } else {
        // SAFETY: the caller's array of `n` items.
        unsafe { core::slice::from_raw_parts(p, n as usize) }
    }
}

/// D2D1_BEZIER_SEGMENT.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Bezier {
    p1: Point,
    p2: Point,
    p3: Point,
}

/// D2D1_QUADRATIC_BEZIER_SEGMENT.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Quadratic {
    p1: Point,
    p2: Point,
}

/// D2D1_ARC_SEGMENT.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Arc {
    point: Point,
    rx: f32,
    ry: f32,
    rotation: f32,
    sweep: u32,
    size: u32,
}

extern "system" fn sink_AddLines(this: Obj, pts: *const Point, n: u32) {
    add_segs(this, items(pts, n).iter().map(|&p| Seg::Line(p)));
}

extern "system" fn sink_AddBeziers(this: Obj, b: *const Bezier, n: u32) {
    add_segs(this, items(b, n).iter().map(|b| Seg::Cubic(b.p1, b.p2, b.p3)));
}

extern "system" fn sink_AddLine(this: Obj, p: Point) {
    add_segs(this, core::iter::once(Seg::Line(p)));
}

extern "system" fn sink_AddBezier(this: Obj, b: *const Bezier) {
    if let Some(b) = read(b) {
        add_segs(this, core::iter::once(Seg::Cubic(b.p1, b.p2, b.p3)));
    }
}

/// The figure's current point (its start or its last segment's end).
fn current_point(this: Obj) -> Option<Point> {
    // SAFETY: one of our sinks.
    let path = unsafe { (*(this as *const Sink)).path };
    path_state(path, |s| {
        let f = s.path.figs.last()?;
        if !s.in_figure {
            return None;
        }
        Some(match f.segs.last() {
            None => f.start,
            Some(Seg::Line(p)) | Some(Seg::Cubic(_, _, p)) => *p,
        })
    })
    .flatten()
}

/// A quadratic Bézier from `p0` as the cubic that traces it.
fn quad_to_cubic(p0: Point, q: &Quadratic) -> Seg {
    let c1 = pt(p0.x + 2.0 / 3.0 * (q.p1.x - p0.x), p0.y + 2.0 / 3.0 * (q.p1.y - p0.y));
    let c2 = pt(q.p2.x + 2.0 / 3.0 * (q.p1.x - q.p2.x), q.p2.y + 2.0 / 3.0 * (q.p1.y - q.p2.y));
    Seg::Cubic(c1, c2, q.p2)
}

extern "system" fn sink_AddQuadraticBezier(this: Obj, q: *const Quadratic) {
    sink_AddQuadraticBeziers(this, q, 1);
}

extern "system" fn sink_AddQuadraticBeziers(this: Obj, q: *const Quadratic, n: u32) {
    for q in items(q, n) {
        let p0 = current_point(this).unwrap_or(q.p1);
        add_segs(this, core::iter::once(quad_to_cubic(p0, q)));
    }
}

/// AddArc: the elliptical arc to `point` as cubic Béziers of at most a
/// quarter turn each (the endpoint-to-centre conversion of the SVG
/// specification's implementation notes, F.6.5); zero radii draw a line.
extern "system" fn sink_AddArc(this: Obj, a: *const Arc) {
    let Some(a) = read(a) else { return };
    let Some(p0) = current_point(this) else {
        add_segs(this, core::iter::once(Seg::Line(a.point)));
        return;
    };
    let mut segs: Vec<Seg> = Vec::new();
    if arc_to_cubics(p0, &a, &mut segs).is_none() {
        segs.clear();
        let _ = push(&mut segs, Seg::Line(a.point));
    }
    add_segs(this, segs.into_iter());
}

fn arc_to_cubics(p0: Point, a: &Arc, out: &mut Vec<Seg>) -> Option<()> {
    let p1 = a.point;
    if p0 == p1 {
        return Some(());
    }
    let (mut rx, mut ry) = (a.rx.abs(), a.ry.abs());
    if rx == 0.0 || ry == 0.0 {
        push(out, Seg::Line(p1)).then_some(())?;
        return Some(());
    }
    let phi = a.rotation * (core::f32::consts::PI / 180.0);
    let (sp, cp) = (libm::sinf(phi), libm::cosf(phi));
    let (dx, dy) = ((p0.x - p1.x) / 2.0, (p0.y - p1.y) / 2.0);
    let x1 = cp * dx + sp * dy;
    let y1 = -sp * dx + cp * dy;
    let lambda = x1 * x1 / (rx * rx) + y1 * y1 / (ry * ry);
    if lambda > 1.0 {
        let k = libm::sqrtf(lambda);
        rx *= k;
        ry *= k;
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut coef = if den > 0.0 { libm::sqrtf((num / den).max(0.0)) } else { 0.0 };
    // D2D1_SWEEP_DIRECTION_CLOCKWISE 1 (SVG's sweep-flag 1 on a y-down
    // screen), D2D1_ARC_SIZE_LARGE 1.
    let clockwise = a.sweep == 1;
    if (a.size == 1) == clockwise {
        coef = -coef;
    }
    let cx1 = coef * rx * y1 / ry;
    let cy1 = -coef * ry * x1 / rx;
    let cx = cp * cx1 - sp * cy1 + (p0.x + p1.x) / 2.0;
    let cy = sp * cx1 + cp * cy1 + (p0.y + p1.y) / 2.0;
    let angle = |ux: f32, uy: f32| libm::atan2f(uy, ux);
    let t1 = angle((x1 - cx1) / rx, (y1 - cy1) / ry);
    let t2 = angle((-x1 - cx1) / rx, (-y1 - cy1) / ry);
    let mut dt = t2 - t1;
    let tau = 2.0 * core::f32::consts::PI;
    if clockwise && dt < 0.0 {
        dt += tau;
    } else if !clockwise && dt > 0.0 {
        dt -= tau;
    }
    let n = libm::ceilf(dt.abs() / (core::f32::consts::PI / 2.0) - 1e-4).max(1.0) as usize;
    let step = dt / n as f32;
    let k = 4.0 / 3.0 * libm::tanf(step / 4.0);
    let on = |t: f32| {
        let (x, y) = (rx * libm::cosf(t), ry * libm::sinf(t));
        pt(cp * x - sp * y + cx, sp * x + cp * y + cy)
    };
    let tangent = |t: f32| {
        let (x, y) = (-rx * libm::sinf(t), ry * libm::cosf(t));
        pt(cp * x - sp * y, sp * x + cp * y)
    };
    let mut t = t1;
    let mut from = p0;
    for i in 0..n {
        let t_next = t + step;
        let to = if i == n - 1 { p1 } else { on(t_next) };
        let (d0, d1) = (tangent(t), tangent(t_next));
        let c1 = pt(from.x + k * d0.x, from.y + k * d0.y);
        let c2 = pt(to.x - k * d1.x, to.y - k * d1.y);
        push(out, Seg::Cubic(c1, c2, to)).then_some(())?;
        from = to;
        t = t_next;
    }
    Some(())
}
