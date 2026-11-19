//! The factory (ID2D1Factory, ID2D1Factory1 and ID2D1Multithread on the
//! same object) and stroke styles.
//!
//! Each D2D1CreateFactory makes a new factory; every resource holds a
//! reference to its factory (GetFactory). ID2D1Multithread answers on
//! single- and multi-threaded factories alike (Windows 10's Direct2D
//! does: Wine's test_mt_factory); a multi-threaded factory's Enter and
//! Leave hold a critical section of its own, a single-threaded one's do
//! nothing.

use core::cell::UnsafeCell;

use alloc::vec::Vec;

use crate::geometry::{self, Ellipse, RoundedRect, Shape};
use crate::raster::StrokeStyle;
use crate::{
    E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, Guid, HResult, Header, IID_FACTORY, IID_FACTORY1,
    IID_FACTORY2, IID_IUNKNOWN, IID_MULTITHREAD, IID_RESOURCE, IID_STROKE_STYLE, Matrix, Obj,
    RectF, Res, S_OK, VTable, add_ref, gap, give, not_implemented, put, query, read,
    release_object, try_box, vtable_of,
};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn InitializeCriticalSection(cs: *mut u8);
    fn DeleteCriticalSection(cs: *mut u8);
    fn EnterCriticalSection(cs: *mut u8);
    fn LeaveCriticalSection(cs: *mut u8);
}

#[repr(C)]
pub struct Factory {
    hdr: Header,
    /// ID2D1Multithread's vtable: the interface pointer is its address.
    mt_vtbl: *const (),
    multithreaded: bool,
    /// The CRITICAL_SECTION of a multi-threaded factory (40 bytes on x64).
    cs: UnsafeCell<[u64; 6]>,
}

impl Drop for Factory {
    fn drop(&mut self) {
        if self.multithreaded {
            // SAFETY: initialized in new_factory, no longer entered.
            unsafe { DeleteCriticalSection(self.cs.get().cast()) };
        }
    }
}

/// ID2D1Factory, and ID2D1Factory1's ten further methods after it
/// (d2d1_1.h). CreateDevice is the one Office needs: everything else of
/// 1.1 hangs off the device.
// The interfaces this module's vtable slots belong to: `cargo xtask
// gen-trace` names each wrapper's method from them (xtask/src/gentrace.rs).
// gen-trace prefix f_ ID2D1Factory
// gen-trace prefix f1_ ID2D1Factory1
// gen-trace prefix mt_ ID2D1Multithread
// gen-trace prefix s_ ID2D1StrokeStyle
static FACTORY_VTBL: VTable<28> = VTable([
    factory_query_fn as *const (),
    add_ref as *const (),
    factory_release_fn as *const (),
    traced::f_ReloadSystemMetrics as *const (),
    traced::f_GetDesktopDpi as *const (),
    traced::f_CreateRectangleGeometry as *const (),
    traced::f_CreateRoundedRectangleGeometry as *const (),
    traced::f_CreateEllipseGeometry as *const (),
    traced::f_CreateGeometryGroup as *const (),
    traced::f_CreateTransformedGeometry as *const (),
    traced::f_CreatePathGeometry as *const (),
    traced::f_CreateStrokeStyle as *const (),
    traced::f_CreateDrawingStateBlock as *const (),
    crate::target::traced::f_CreateWicBitmapRenderTarget as *const (),
    crate::target::traced::f_CreateHwndRenderTarget as *const (),
    crate::target::traced::f_CreateDxgiSurfaceRenderTarget as *const (),
    crate::target::traced::f_CreateDCRenderTarget as *const (),
    crate::device::traced::f1_CreateDevice as *const (),
    traced::f1_CreateStrokeStyle as *const (),
    traced::f1_CreatePathGeometry as *const (),
    traced::f1_CreateDrawingStateBlock as *const (),
    traced::f1_CreateGdiMetafile as *const (),
    traced::f1_RegisterEffectFromStream as *const (),
    traced::f1_RegisterEffectFromString as *const (),
    traced::f1_UnregisterEffect as *const (),
    traced::f1_GetRegisteredEffects as *const (),
    traced::f1_GetEffectProperties as *const (),
    // ID2D1Factory2::CreateDevice gives an ID2D1Device1, which is
    // what CreateDevice already makes: one device object answers both
    // interfaces (`device`).
    crate::device::traced::f2_CreateDevice as *const (),
]);

static MT_VTBL: VTable<6> = VTable([
    mt_query as *const (),
    mt_add_ref as *const (),
    mt_release as *const (),
    traced::mt_GetMultithreadProtected as *const (),
    traced::mt_Enter as *const (),
    traced::mt_Leave as *const (),
]);

/// A new factory with one reference.
pub fn new_factory(multithreaded: bool) -> Option<Obj> {
    let f = try_box(Factory {
        hdr: Header::new(FACTORY_VTBL.addr()),
        mt_vtbl: MT_VTBL.addr(),
        multithreaded,
        cs: UnsafeCell::new([0; 6]),
    })?;
    if multithreaded {
        // SAFETY: the factory's own storage, large enough for one.
        unsafe { InitializeCriticalSection((*f).cs.get().cast()) };
    }
    Some(f.cast())
}

fn factory<'a>(o: Obj) -> &'a Factory {
    // SAFETY: one of our factories.
    unsafe { &*(o as *const Factory) }
}

/// The factory behind its ID2D1Multithread pointer.
fn from_mt(mt: Obj) -> Obj {
    (mt as usize - core::mem::offset_of!(Factory, mt_vtbl)) as Obj
}

pub fn factory_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    if read(riid) == Some(IID_MULTITHREAD) {
        add_ref(this);
        put(out, (&factory(this).mt_vtbl as *const *const ()).cast_mut().cast());
        return S_OK;
    }
    query(this, b"ID2D1Factory", &[IID_FACTORY, IID_FACTORY1, IID_FACTORY2], riid, out)
}

pub fn factory_release(this: Obj) -> u32 {
    release_object::<Factory>(this)
}

extern "system" fn factory_release_fn(this: Obj) -> u32 {
    factory_release(this)
}

extern "system" fn mt_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    // IUnknown is the factory's own pointer (COM's identity rule).
    if read(riid) == Some(IID_IUNKNOWN) {
        add_ref(from_mt(this));
        put(out, from_mt(this));
        return S_OK;
    }
    factory_query(from_mt(this), riid, out)
}

extern "system" fn mt_add_ref(this: Obj) -> u32 {
    add_ref(from_mt(this))
}

extern "system" fn mt_release(this: Obj) -> u32 {
    factory_release(from_mt(this))
}

extern "system" fn mt_GetMultithreadProtected(this: Obj) -> i32 {
    i32::from(factory(from_mt(this)).multithreaded)
}

extern "system" fn mt_Enter(this: Obj) {
    let f = factory(from_mt(this));
    if f.multithreaded {
        // SAFETY: the factory's initialized critical section.
        unsafe { EnterCriticalSection(f.cs.get().cast()) };
    }
}

extern "system" fn mt_Leave(this: Obj) {
    let f = factory(from_mt(this));
    if f.multithreaded {
        // SAFETY: as above; the caller entered it.
        unsafe { LeaveCriticalSection(f.cs.get().cast()) };
    }
}

extern "system" fn factory_query_fn(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    factory_query(this, riid, out)
}

extern "system" fn f_ReloadSystemMetrics(_this: Obj) -> HResult {
    S_OK
}

/// GetDesktopDpi: gala's screen is 96 DPI (user32's GetDpiForSystem).
extern "system" fn f_GetDesktopDpi(_this: Obj, x: *mut f32, y: *mut f32) {
    put(x, 96.0);
    put(y, 96.0);
}

extern "system" fn f_CreateRectangleGeometry(this: Obj, r: *const RectF, out: *mut Obj) -> HResult {
    let Some(r) = read(r) else { return E_INVALIDARG };
    geometry::create(this, Shape::Rect(r), out)
}

extern "system" fn f_CreateRoundedRectangleGeometry(
    this: Obj,
    r: *const RoundedRect,
    out: *mut Obj,
) -> HResult {
    let Some(r) = read(r) else { return E_INVALIDARG };
    geometry::create(this, Shape::Rounded(r.rect, r.rx, r.ry), out)
}

extern "system" fn f_CreateEllipseGeometry(this: Obj, e: *const Ellipse, out: *mut Obj) -> HResult {
    let Some(e) = read(e) else { return E_INVALIDARG };
    geometry::create(this, Shape::Ellipse(e.center, e.rx, e.ry), out)
}

extern "system" fn f_CreateGeometryGroup(
    _this: Obj,
    _mode: u32,
    _geometries: *const Obj,
    _count: u32,
    out: *mut Obj,
) -> HResult {
    put(out, core::ptr::null_mut());
    not_implemented(b"ID2D1Factory::CreateGeometryGroup")
}

extern "system" fn f_CreateTransformedGeometry(
    _this: Obj,
    _source: Obj,
    _m: *const Matrix,
    out: *mut Obj,
) -> HResult {
    put(out, core::ptr::null_mut());
    not_implemented(b"ID2D1Factory::CreateTransformedGeometry")
}

extern "system" fn f_CreatePathGeometry(this: Obj, out: *mut Obj) -> HResult {
    geometry::create(this, geometry::new_path_shape(), out)
}

extern "system" fn f_CreateDrawingStateBlock(
    this: Obj,
    desc: *const crate::state::StateDesc,
    params: Obj,
    out: *mut Obj,
) -> HResult {
    crate::state::create(this, desc, params, out)
}

// ---- ID2D1Factory1's own methods ----------------------------------------------
//
// CreateDevice is in `device`. The rest are recorded and E_NOTIMPL: a
// 1.1 stroke style (ID2D1StrokeStyle1) and path geometry
// (ID2D1PathGeometry1) each add one method to the 1.0 interface, which
// is work for the program that asks for them, and effects are a
// rendering graph gala has none of.

extern "system" fn f1_CreateStrokeStyle(
    _this: Obj,
    _props: *const u8,
    _dashes: *const f32,
    _count: u32,
    out: *mut Obj,
) -> HResult {
    put(out, core::ptr::null_mut());
    not_implemented(b"ID2D1Factory1::CreateStrokeStyle")
}

extern "system" fn f1_CreatePathGeometry(_this: Obj, out: *mut Obj) -> HResult {
    put(out, core::ptr::null_mut());
    not_implemented(b"ID2D1Factory1::CreatePathGeometry")
}

extern "system" fn f1_CreateDrawingStateBlock(
    this: Obj,
    desc: *const crate::state::StateDesc1,
    params: Obj,
    out: *mut Obj,
) -> HResult {
    crate::state::create1(this, desc, params, out)
}

extern "system" fn f1_CreateGdiMetafile(_this: Obj, _stream: Obj, out: *mut Obj) -> HResult {
    put(out, core::ptr::null_mut());
    not_implemented(b"ID2D1Factory1::CreateGdiMetafile")
}

extern "system" fn f1_RegisterEffectFromStream(
    _this: Obj,
    class_id: *const Guid,
    _xml: Obj,
    _bindings: *const u8,
    _count: u32,
    _factory: *const (),
) -> HResult {
    gap(b"ID2D1Factory1::RegisterEffectFromStream", read(class_id).map_or(0, |g| crate::data1(&g)));
    crate::E_NOTIMPL
}

extern "system" fn f1_RegisterEffectFromString(
    _this: Obj,
    class_id: *const Guid,
    _xml: *const u16,
    _bindings: *const u8,
    _count: u32,
    _factory: *const (),
) -> HResult {
    gap(b"ID2D1Factory1::RegisterEffectFromString", read(class_id).map_or(0, |g| crate::data1(&g)));
    crate::E_NOTIMPL
}

extern "system" fn f1_UnregisterEffect(_this: Obj, _class_id: *const Guid) -> HResult {
    not_implemented(b"ID2D1Factory1::UnregisterEffect")
}

/// GetRegisteredEffects: gala registers none, and says so (Direct2D's
/// own built-in effects are not counted here either).
extern "system" fn f1_GetRegisteredEffects(
    _this: Obj,
    _effects: *mut Guid,
    _count: u32,
    returned: *mut u32,
    registered: *mut u32,
) -> HResult {
    put(returned, 0);
    put(registered, 0);
    S_OK
}

extern "system" fn f1_GetEffectProperties(
    _this: Obj,
    class_id: *const Guid,
    out: *mut Obj,
) -> HResult {
    put(out, core::ptr::null_mut());
    gap(b"ID2D1Factory1::GetEffectProperties", read(class_id).map_or(0, |g| crate::data1(&g)));
    crate::E_NOTIMPL
}

// ---- Stroke styles ------------------------------------------------------------

/// D2D1_STROKE_STYLE_PROPERTIES.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StrokeProps {
    start_cap: u32,
    end_cap: u32,
    dash_cap: u32,
    line_join: u32,
    miter_limit: f32,
    dash_style: u32,
    dash_offset: f32,
}

#[repr(C)]
struct Stroke {
    res: Res,
    props: StrokeProps,
    dashes: Vec<f32>,
}

static STROKE_VTBL: VTable<13> = VTable([
    stroke_query as *const (),
    add_ref as *const (),
    stroke_release as *const (),
    crate::traced::get_factory as *const (),
    traced::s_GetStartCap as *const (),
    traced::s_GetEndCap as *const (),
    traced::s_GetDashCap as *const (),
    traced::s_GetMiterLimit as *const (),
    traced::s_GetLineJoin as *const (),
    traced::s_GetDashOffset as *const (),
    traced::s_GetDashStyle as *const (),
    traced::s_GetDashesCount as *const (),
    traced::s_GetDashes as *const (),
]);

const DASH_SOLID: u32 = 0;
const DASH_CUSTOM: u32 = 5;

/// CreateStrokeStyle: caps, join and miter limit are drawn; dashes are
/// kept and answered but drawn solid (recorded). Custom dashes need their
/// array; dashes given for another style are E_INVALIDARG.
extern "system" fn f_CreateStrokeStyle(
    this: Obj,
    props: *const StrokeProps,
    dashes: *const f32,
    count: u32,
    out: *mut Obj,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let Some(p) = read(props) else { return E_INVALIDARG };
    if p.dash_style > DASH_CUSTOM
        || p.start_cap > 3
        || p.end_cap > 3
        || p.dash_cap > 3
        || p.line_join > 3
    {
        return E_INVALIDARG;
    }
    let custom = p.dash_style == DASH_CUSTOM;
    if custom && (dashes.is_null() || count == 0) || !custom && !dashes.is_null() {
        return E_INVALIDARG;
    }
    let mut v = Vec::new();
    if custom {
        if v.try_reserve_exact(count as usize).is_err() {
            return E_OUTOFMEMORY;
        }
        // SAFETY: the caller's `count` dashes.
        v.extend_from_slice(unsafe { core::slice::from_raw_parts(dashes, count as usize) });
    }
    if p.dash_style != DASH_SOLID {
        gap(b"ID2D1StrokeStyle(dashes, drawn solid)", p.dash_style.into());
    }
    give(out, Stroke { res: Res::new(STROKE_VTBL.addr(), this), props: p, dashes: v })
}

fn stroke<'a>(o: Obj) -> &'a Stroke {
    // SAFETY: one of our stroke styles.
    unsafe { &*(o as *const Stroke) }
}

/// The raster style of a draw call's stroke style argument (the default
/// for NULL, or for one that is not ours).
pub fn stroke_style_of(o: Obj) -> StrokeStyle {
    if o.is_null() || vtable_of(o) != STROKE_VTBL.addr() {
        if !o.is_null() {
            gap(b"draw(a stroke style not made by gala's d2d1)", 0);
        }
        return StrokeStyle::DEFAULT;
    }
    let p = stroke(o).props;
    StrokeStyle {
        start_cap: p.start_cap,
        end_cap: p.end_cap,
        join: p.line_join,
        miter_limit: p.miter_limit,
    }
}

extern "system" fn stroke_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    query(this, b"ID2D1StrokeStyle", &[IID_RESOURCE, IID_STROKE_STYLE], riid, out)
}

extern "system" fn stroke_release(this: Obj) -> u32 {
    release_object::<Stroke>(this)
}

extern "system" fn s_GetStartCap(this: Obj) -> u32 {
    stroke(this).props.start_cap
}

extern "system" fn s_GetEndCap(this: Obj) -> u32 {
    stroke(this).props.end_cap
}

extern "system" fn s_GetDashCap(this: Obj) -> u32 {
    stroke(this).props.dash_cap
}

extern "system" fn s_GetMiterLimit(this: Obj) -> f32 {
    stroke(this).props.miter_limit
}

extern "system" fn s_GetLineJoin(this: Obj) -> u32 {
    stroke(this).props.line_join
}

extern "system" fn s_GetDashOffset(this: Obj) -> f32 {
    stroke(this).props.dash_offset
}

extern "system" fn s_GetDashStyle(this: Obj) -> u32 {
    stroke(this).props.dash_style
}

extern "system" fn s_GetDashesCount(this: Obj) -> u32 {
    stroke(this).dashes.len() as u32
}

extern "system" fn s_GetDashes(this: Obj, out: *mut f32, n: u32) {
    if out.is_null() {
        return;
    }
    for (i, d) in stroke(this).dashes.iter().take(n as usize).enumerate() {
        // SAFETY: the caller's array of `n` floats.
        unsafe { out.add(i).write_unaligned(*d) };
    }
}

// ==== generated by `cargo xtask gen-trace`: do not edit ====
/// The wrapper of every vtable slot of this module, each recording
/// the first call that reaches its method (`cargo xtask gen-trace`).
pub(crate) mod traced {
    use super::*;
    /// ID2D1Factory1::CreateDrawingStateBlock, traced.
    pub(super) extern "system" fn f1_CreateDrawingStateBlock(
        this: Obj,
        desc: *const crate::state::StateDesc1,
        params: Obj,
        out: *mut Obj,
    ) -> HResult {
        crate::reached!(b"ID2D1Factory1::CreateDrawingStateBlock");
        super::f1_CreateDrawingStateBlock(this, desc, params, out)
    }
    /// ID2D1Factory1::CreateGdiMetafile, traced.
    pub(super) extern "system" fn f1_CreateGdiMetafile(
        _this: Obj,
        _stream: Obj,
        out: *mut Obj,
    ) -> HResult {
        crate::reached!(b"ID2D1Factory1::CreateGdiMetafile");
        super::f1_CreateGdiMetafile(_this, _stream, out)
    }
    /// ID2D1Factory1::CreatePathGeometry, traced.
    pub(super) extern "system" fn f1_CreatePathGeometry(_this: Obj, out: *mut Obj) -> HResult {
        crate::reached!(b"ID2D1Factory1::CreatePathGeometry");
        super::f1_CreatePathGeometry(_this, out)
    }
    /// ID2D1Factory1::CreateStrokeStyle, traced.
    pub(super) extern "system" fn f1_CreateStrokeStyle(
        _this: Obj,
        _props: *const u8,
        _dashes: *const f32,
        _count: u32,
        out: *mut Obj,
    ) -> HResult {
        crate::reached!(b"ID2D1Factory1::CreateStrokeStyle");
        super::f1_CreateStrokeStyle(_this, _props, _dashes, _count, out)
    }
    /// ID2D1Factory1::GetEffectProperties, traced.
    pub(super) extern "system" fn f1_GetEffectProperties(
        _this: Obj,
        class_id: *const Guid,
        out: *mut Obj,
    ) -> HResult {
        crate::reached!(b"ID2D1Factory1::GetEffectProperties");
        super::f1_GetEffectProperties(_this, class_id, out)
    }
    /// ID2D1Factory1::GetRegisteredEffects, traced.
    pub(super) extern "system" fn f1_GetRegisteredEffects(
        _this: Obj,
        _effects: *mut Guid,
        _count: u32,
        returned: *mut u32,
        registered: *mut u32,
    ) -> HResult {
        crate::reached!(b"ID2D1Factory1::GetRegisteredEffects");
        super::f1_GetRegisteredEffects(_this, _effects, _count, returned, registered)
    }
    /// ID2D1Factory1::RegisterEffectFromStream, traced.
    pub(super) extern "system" fn f1_RegisterEffectFromStream(
        _this: Obj,
        class_id: *const Guid,
        _xml: Obj,
        _bindings: *const u8,
        _count: u32,
        _factory: *const (),
    ) -> HResult {
        crate::reached!(b"ID2D1Factory1::RegisterEffectFromStream");
        super::f1_RegisterEffectFromStream(_this, class_id, _xml, _bindings, _count, _factory)
    }
    /// ID2D1Factory1::RegisterEffectFromString, traced.
    pub(super) extern "system" fn f1_RegisterEffectFromString(
        _this: Obj,
        class_id: *const Guid,
        _xml: *const u16,
        _bindings: *const u8,
        _count: u32,
        _factory: *const (),
    ) -> HResult {
        crate::reached!(b"ID2D1Factory1::RegisterEffectFromString");
        super::f1_RegisterEffectFromString(_this, class_id, _xml, _bindings, _count, _factory)
    }
    /// ID2D1Factory1::UnregisterEffect, traced.
    pub(super) extern "system" fn f1_UnregisterEffect(
        _this: Obj,
        _class_id: *const Guid,
    ) -> HResult {
        crate::reached!(b"ID2D1Factory1::UnregisterEffect");
        super::f1_UnregisterEffect(_this, _class_id)
    }
    /// ID2D1Factory::CreateDrawingStateBlock, traced.
    pub(super) extern "system" fn f_CreateDrawingStateBlock(
        this: Obj,
        desc: *const crate::state::StateDesc,
        params: Obj,
        out: *mut Obj,
    ) -> HResult {
        crate::reached!(b"ID2D1Factory::CreateDrawingStateBlock");
        super::f_CreateDrawingStateBlock(this, desc, params, out)
    }
    /// ID2D1Factory::CreateEllipseGeometry, traced.
    pub(super) extern "system" fn f_CreateEllipseGeometry(
        this: Obj,
        e: *const Ellipse,
        out: *mut Obj,
    ) -> HResult {
        crate::reached!(b"ID2D1Factory::CreateEllipseGeometry");
        super::f_CreateEllipseGeometry(this, e, out)
    }
    /// ID2D1Factory::CreateGeometryGroup, traced.
    pub(super) extern "system" fn f_CreateGeometryGroup(
        _this: Obj,
        _mode: u32,
        _geometries: *const Obj,
        _count: u32,
        out: *mut Obj,
    ) -> HResult {
        crate::reached!(b"ID2D1Factory::CreateGeometryGroup");
        super::f_CreateGeometryGroup(_this, _mode, _geometries, _count, out)
    }
    /// ID2D1Factory::CreatePathGeometry, traced.
    pub(super) extern "system" fn f_CreatePathGeometry(this: Obj, out: *mut Obj) -> HResult {
        crate::reached!(b"ID2D1Factory::CreatePathGeometry");
        super::f_CreatePathGeometry(this, out)
    }
    /// ID2D1Factory::CreateRectangleGeometry, traced.
    pub(super) extern "system" fn f_CreateRectangleGeometry(
        this: Obj,
        r: *const RectF,
        out: *mut Obj,
    ) -> HResult {
        crate::reached!(b"ID2D1Factory::CreateRectangleGeometry");
        super::f_CreateRectangleGeometry(this, r, out)
    }
    /// ID2D1Factory::CreateRoundedRectangleGeometry, traced.
    pub(super) extern "system" fn f_CreateRoundedRectangleGeometry(
        this: Obj,
        r: *const RoundedRect,
        out: *mut Obj,
    ) -> HResult {
        crate::reached!(b"ID2D1Factory::CreateRoundedRectangleGeometry");
        super::f_CreateRoundedRectangleGeometry(this, r, out)
    }
    /// ID2D1Factory::CreateStrokeStyle, traced.
    pub(super) extern "system" fn f_CreateStrokeStyle(
        this: Obj,
        props: *const StrokeProps,
        dashes: *const f32,
        count: u32,
        out: *mut Obj,
    ) -> HResult {
        crate::reached!(b"ID2D1Factory::CreateStrokeStyle");
        super::f_CreateStrokeStyle(this, props, dashes, count, out)
    }
    /// ID2D1Factory::CreateTransformedGeometry, traced.
    pub(super) extern "system" fn f_CreateTransformedGeometry(
        _this: Obj,
        _source: Obj,
        _m: *const Matrix,
        out: *mut Obj,
    ) -> HResult {
        crate::reached!(b"ID2D1Factory::CreateTransformedGeometry");
        super::f_CreateTransformedGeometry(_this, _source, _m, out)
    }
    /// ID2D1Factory::GetDesktopDpi, traced.
    pub(super) extern "system" fn f_GetDesktopDpi(_this: Obj, x: *mut f32, y: *mut f32) {
        crate::reached!(b"ID2D1Factory::GetDesktopDpi");
        super::f_GetDesktopDpi(_this, x, y)
    }
    /// ID2D1Factory::ReloadSystemMetrics, traced.
    pub(super) extern "system" fn f_ReloadSystemMetrics(_this: Obj) -> HResult {
        crate::reached!(b"ID2D1Factory::ReloadSystemMetrics");
        super::f_ReloadSystemMetrics(_this)
    }
    /// ID2D1Multithread::Enter, traced.
    pub(super) extern "system" fn mt_Enter(this: Obj) {
        crate::reached!(b"ID2D1Multithread::Enter");
        super::mt_Enter(this)
    }
    /// ID2D1Multithread::GetMultithreadProtected, traced.
    pub(super) extern "system" fn mt_GetMultithreadProtected(this: Obj) -> i32 {
        crate::reached!(b"ID2D1Multithread::GetMultithreadProtected");
        super::mt_GetMultithreadProtected(this)
    }
    /// ID2D1Multithread::Leave, traced.
    pub(super) extern "system" fn mt_Leave(this: Obj) {
        crate::reached!(b"ID2D1Multithread::Leave");
        super::mt_Leave(this)
    }
    /// ID2D1StrokeStyle::GetDashCap, traced.
    pub(super) extern "system" fn s_GetDashCap(this: Obj) -> u32 {
        crate::reached!(b"ID2D1StrokeStyle::GetDashCap");
        super::s_GetDashCap(this)
    }
    /// ID2D1StrokeStyle::GetDashOffset, traced.
    pub(super) extern "system" fn s_GetDashOffset(this: Obj) -> f32 {
        crate::reached!(b"ID2D1StrokeStyle::GetDashOffset");
        super::s_GetDashOffset(this)
    }
    /// ID2D1StrokeStyle::GetDashStyle, traced.
    pub(super) extern "system" fn s_GetDashStyle(this: Obj) -> u32 {
        crate::reached!(b"ID2D1StrokeStyle::GetDashStyle");
        super::s_GetDashStyle(this)
    }
    /// ID2D1StrokeStyle::GetDashes, traced.
    pub(super) extern "system" fn s_GetDashes(this: Obj, out: *mut f32, n: u32) {
        crate::reached!(b"ID2D1StrokeStyle::GetDashes");
        super::s_GetDashes(this, out, n)
    }
    /// ID2D1StrokeStyle::GetDashesCount, traced.
    pub(super) extern "system" fn s_GetDashesCount(this: Obj) -> u32 {
        crate::reached!(b"ID2D1StrokeStyle::GetDashesCount");
        super::s_GetDashesCount(this)
    }
    /// ID2D1StrokeStyle::GetEndCap, traced.
    pub(super) extern "system" fn s_GetEndCap(this: Obj) -> u32 {
        crate::reached!(b"ID2D1StrokeStyle::GetEndCap");
        super::s_GetEndCap(this)
    }
    /// ID2D1StrokeStyle::GetLineJoin, traced.
    pub(super) extern "system" fn s_GetLineJoin(this: Obj) -> u32 {
        crate::reached!(b"ID2D1StrokeStyle::GetLineJoin");
        super::s_GetLineJoin(this)
    }
    /// ID2D1StrokeStyle::GetMiterLimit, traced.
    pub(super) extern "system" fn s_GetMiterLimit(this: Obj) -> f32 {
        crate::reached!(b"ID2D1StrokeStyle::GetMiterLimit");
        super::s_GetMiterLimit(this)
    }
    /// ID2D1StrokeStyle::GetStartCap, traced.
    pub(super) extern "system" fn s_GetStartCap(this: Obj) -> u32 {
        crate::reached!(b"ID2D1StrokeStyle::GetStartCap");
        super::s_GetStartCap(this)
    }
}
