//! gala's d2d1.dll: Direct2D (d2d1.h) drawn in software, the part Office
//! draws its documents and user interface with (ROADMAP OF; ARCHITECTURE
//! "Direct2D").
//!
//! - The factory (`factory`): D2D1CreateFactory, single- or
//!   multi-threaded (ID2D1Multithread), geometries, stroke styles and the
//!   render targets.
//! - Render targets (`target`): over a WIC bitmap
//!   (CreateWicBitmapRenderTarget), a GDI device context
//!   (CreateDCRenderTarget, BindDC) and a window (CreateHwndRenderTarget,
//!   presented through GDI). Every target draws in software into 32 bpp
//!   premultiplied pixels; a request for hardware rendering is served by
//!   the same rasterizer (gala has no Direct3D 11 device, ROADMAP GPU).
//! - Brushes, gradient stop collections and bitmaps (`brush`).
//! - Geometries: rectangles, rounded rectangles, ellipses, paths and their
//!   sinks (`geometry`).
//! - The rasterizer (`raster`): figures flattened to polygons, coverage
//!   on a 4 x 4 sample grid per pixel (antialiased) or at the pixel's
//!   centre (aliased), the fill rule, strokes as polygons, axis-aligned
//!   clips, and source-over compositing of a paint (a colour, a gradient
//!   or a bitmap).
//!
//! What it does not do is recorded in the ledger (`d2d1.dll!...`): text
//! (DrawText, DrawTextLayout, DrawGlyphRun: gala's DirectWrite has no
//! text layout yet), layers, meshes, opacity masks, drawing state blocks,
//! compatible render targets, DXGI surface targets, dashes, ID2D1Factory1
//! and device contexts, and the GDI interop interface.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;

mod brush;
mod factory;
mod geometry;
mod raster;
mod target;

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
pub const E_NOTIMPL: HResult = 0x8000_4001_u32 as i32;
pub const E_NOINTERFACE: HResult = 0x8000_4002_u32 as i32;
pub const E_POINTER: HResult = 0x8000_4003_u32 as i32;
pub const E_OUTOFMEMORY: HResult = 0x8007_000e_u32 as i32;
pub const E_INVALIDARG: HResult = 0x8007_0057_u32 as i32;
pub const D2DERR_WRONG_STATE: HResult = 0x8899_0001_u32 as i32;
pub const D2DERR_NOT_INITIALIZED: HResult = 0x8899_0002_u32 as i32;
pub const D2DERR_PUSH_POP_UNBALANCED: HResult = 0x8899_0016_u32 as i32;
/// WINCODEC_ERR_UNSUPPORTEDPIXELFORMAT (d2derr.h).
pub const D2DERR_UNSUPPORTED_PIXEL_FORMAT: HResult = 0x8898_2f80_u32 as i32;

/// The ledger's kinds (GalaRecordMissing's codes).
const LEDGER_UNSUPPORTED: u32 = 2;
const LEDGER_NOT_IMPLEMENTED: u32 = 3;
const LEDGER_OTHER: u32 = 5;

fn record_kind(what: &[u8], kind: u32, detail: u64) {
    let mut name = [0u8; 96];
    let mut n = 0;
    for &b in b"d2d1.dll!".iter().chain(what) {
        if n == name.len() {
            break;
        }
        name[n] = b;
        n += 1;
    }
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(name.as_ptr(), n, kind, detail) };
}

/// Records `d2d1.dll!<what>`: something this DLL does not support (an
/// option, a case), with a detail.
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
/// The Direct2D interfaces whose IIDs share their last ten bytes
/// (2cd906xx-12e2-11dc-9fed-001143a055f9).
pub const fn d2d_iid(d1: u32) -> Guid {
    guid(d1, 0x12e2, 0x11dc, [0x9f, 0xed, 0x00, 0x11, 0x43, 0xa0, 0x55, 0xf9])
}
pub const IID_RESOURCE: Guid = d2d_iid(0x2cd9_0691);
pub const IID_STROKE_STYLE: Guid = d2d_iid(0x2cd9_069d);
pub const IID_SIMPLIFIED_SINK: Guid = d2d_iid(0x2cd9_069e);
pub const IID_GEOMETRY_SINK: Guid = d2d_iid(0x2cd9_069f);
pub const IID_GEOMETRY: Guid = d2d_iid(0x2cd9_06a1);
pub const IID_RECTANGLE_GEOMETRY: Guid = d2d_iid(0x2cd9_06a2);
pub const IID_ROUNDED_RECTANGLE_GEOMETRY: Guid = d2d_iid(0x2cd9_06a3);
pub const IID_ELLIPSE_GEOMETRY: Guid = d2d_iid(0x2cd9_06a4);
pub const IID_PATH_GEOMETRY: Guid = d2d_iid(0x2cd9_06a5);
pub const IID_GRADIENT_STOPS: Guid = d2d_iid(0x2cd9_06a7);
pub const IID_BRUSH: Guid = d2d_iid(0x2cd9_06a8);
pub const IID_SOLID_BRUSH: Guid = d2d_iid(0x2cd9_06a9);
pub const IID_BITMAP_BRUSH: Guid = d2d_iid(0x2cd9_06aa);
pub const IID_LINEAR_BRUSH: Guid = d2d_iid(0x2cd9_06ab);
pub const IID_RADIAL_BRUSH: Guid = d2d_iid(0x2cd9_06ac);
pub const IID_RENDER_TARGET: Guid = d2d_iid(0x2cd9_0694);
pub const IID_HWND_RENDER_TARGET: Guid = d2d_iid(0x2cd9_0698);
pub const IID_DC_RENDER_TARGET: Guid =
    guid(0x1c51_bc64, 0xde61, 0x46fd, [0x98, 0x99, 0x63, 0xa5, 0xd8, 0xf0, 0x39, 0x50]);
pub const IID_IMAGE: Guid =
    guid(0x6501_9f75, 0x8da2, 0x497c, [0xb3, 0x2c, 0xdf, 0xa3, 0x4e, 0x48, 0xed, 0xe6]);
pub const IID_BITMAP: Guid =
    guid(0xa229_6057, 0xea42, 0x4099, [0x98, 0x3b, 0x53, 0x9f, 0xb6, 0x50, 0x54, 0x26]);
pub const IID_FACTORY: Guid =
    guid(0x0615_2247, 0x6f50, 0x465a, [0x92, 0x45, 0x11, 0x8b, 0xfd, 0x3b, 0x60, 0x07]);
pub const IID_MULTITHREAD: Guid =
    guid(0x31e6_e7bc, 0xe0ff, 0x4d46, [0x8c, 0x64, 0xa0, 0xa8, 0xc4, 0x1c, 0x15, 0xd3]);

/// Reads the caller's GUID (None for NULL).
pub fn read_guid(g: *const Guid) -> Option<Guid> {
    if g.is_null() {
        None
    } else {
        // SAFETY: the caller's GUID.
        Some(unsafe { g.read_unaligned() })
    }
}

/// Reads the caller's value (None for NULL).
pub fn read<T>(p: *const T) -> Option<T> {
    if p.is_null() {
        None
    } else {
        // SAFETY: the caller's value of type T.
        Some(unsafe { p.read_unaligned() })
    }
}

/// Stores `v` at the caller's output, if any.
pub fn put<T>(out: *mut T, v: T) {
    if !out.is_null() {
        // SAFETY: the caller's output.
        unsafe { out.write_unaligned(v) };
    }
}

/// Appends `v` to `vec`, reserving first (false when out of memory).
pub fn push<T>(vec: &mut alloc::vec::Vec<T>, v: T) -> bool {
    if vec.try_reserve(1).is_err() {
        return false;
    }
    vec.push(v);
    true
}

/// A vector of `n` copies of `v` (None when out of memory).
pub fn filled<T: Clone>(n: usize, v: T) -> Option<alloc::vec::Vec<T>> {
    let mut out = alloc::vec::Vec::new();
    out.try_reserve_exact(n).ok()?;
    out.resize(n, v);
    Some(out)
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

/// Boxes `v` and hands it to the caller's `out` (E_OUTOFMEMORY when
/// there is no memory, `out` then NULL).
pub fn give<T>(out: *mut Obj, v: T) -> HResult {
    match try_box(v) {
        Some(p) => {
            put(out, p.cast());
            S_OK
        }
        None => {
            put(out, core::ptr::null_mut());
            E_OUTOFMEMORY
        }
    }
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
    if o.is_null() {
        return core::ptr::null();
    }
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

/// QueryInterface of an object answering `iids` and IUnknown. An
/// interface it does not answer is recorded (`<what> QueryInterface`,
/// the IID's first dword).
pub fn query(this: Obj, what: &[u8], iids: &[Guid], riid: *const Guid, out: *mut Obj) -> HResult {
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
        let mut name = [0u8; 64];
        let mut n = 0;
        for &b in what.iter().chain(b" QueryInterface") {
            if n < name.len() {
                name[n] = b;
                n += 1;
            }
        }
        gap(&name[..n], data1(&iid));
        put(out, core::ptr::null_mut());
        E_NOINTERFACE
    }
}

/// A small spin lock for an object's mutable state. Never held across a
/// call into another object's code that might call back.
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

/// Slot `i` of COM object `o`'s vtable.
pub fn slot(o: Obj, i: usize) -> *const () {
    // SAFETY: a COM object: its first field points at its vtable, which
    // has at least the slots of the interface it was given as.
    unsafe { *(*(o as *const *const *const ())).add(i) }
}

/// IUnknown::AddRef of any COM object.
pub fn com_add_ref(o: Obj) {
    // SAFETY: IUnknown::AddRef's signature.
    let f: extern "system" fn(Obj) -> u32 = unsafe { core::mem::transmute(slot(o, 1)) };
    f(o);
}

/// IUnknown::Release of any COM object.
pub fn com_release(o: Obj) {
    // SAFETY: IUnknown::Release's signature.
    let f: extern "system" fn(Obj) -> u32 = unsafe { core::mem::transmute(slot(o, 2)) };
    f(o);
}

/// A counted reference to a COM object, released when dropped.
pub struct ComRef(pub Obj);
// SAFETY: the objects this DLL keeps references to (factories, bitmaps,
// WIC bitmaps, gradient stops) are free-threaded or used under the
// caller's own threading rules, as Direct2D's are.
unsafe impl Send for ComRef {}

impl ComRef {
    /// A new reference to `o` (NULL stays NULL).
    pub fn new(o: Obj) -> ComRef {
        if !o.is_null() {
            com_add_ref(o);
        }
        ComRef(o)
    }
}

impl Drop for ComRef {
    fn drop(&mut self) {
        if !self.0.is_null() {
            com_release(self.0);
        }
    }
}

/// What every Direct2D resource starts with: the COM header and its
/// factory (ID2D1Resource::GetFactory hands it out).
#[repr(C)]
pub struct Res {
    pub hdr: Header,
    pub factory: ComRef,
}

impl Res {
    pub fn new(vtbl: *const (), factory: Obj) -> Res {
        Res { hdr: Header::new(vtbl), factory: ComRef::new(factory) }
    }
}

/// ID2D1Resource::GetFactory of every resource of this DLL.
pub extern "system" fn get_factory(this: Obj, out: *mut Obj) {
    // SAFETY: one of our resources: it starts with a Res.
    let f = unsafe { (*(this as *const Res)).factory.0 };
    if !f.is_null() {
        com_add_ref(f);
    }
    put(out, f);
}

// ---- Direct2D's types (d2d1.h, dcommon.h) ---------------------------------

/// D2D1_POINT_2F.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

pub const fn pt(x: f32, y: f32) -> Point {
    Point { x, y }
}

/// D2D1_RECT_F.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq)]
pub struct RectF {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

/// D2D1_SIZE_U.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SizeU {
    pub width: u32,
    pub height: u32,
}

/// D2D1_SIZE_F.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SizeF {
    pub width: f32,
    pub height: f32,
}

/// D2D1_COLOR_F (straight alpha).
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq)]
pub struct ColorF {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

/// D2D1_PIXEL_FORMAT.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq)]
pub struct PixelFormat {
    pub format: u32,
    pub alpha: u32,
}

pub const DXGI_FORMAT_UNKNOWN: u32 = 0;
pub const DXGI_FORMAT_R8G8B8A8_UNORM: u32 = 28;
pub const DXGI_FORMAT_B8G8R8A8_UNORM: u32 = 87;
pub const ALPHA_UNKNOWN: u32 = 0;
pub const ALPHA_PREMULTIPLIED: u32 = 1;
pub const ALPHA_STRAIGHT: u32 = 2;
pub const ALPHA_IGNORE: u32 = 3;

/// D2D1_MATRIX_3X2_F: a point (x, y) goes to (x m11 + y m21 + dx,
/// x m12 + y m22 + dy).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Matrix {
    pub m11: f32,
    pub m12: f32,
    pub m21: f32,
    pub m22: f32,
    pub dx: f32,
    pub dy: f32,
}

impl Matrix {
    pub const IDENTITY: Matrix =
        Matrix { m11: 1.0, m12: 0.0, m21: 0.0, m22: 1.0, dx: 0.0, dy: 0.0 };

    pub const fn scale(sx: f32, sy: f32) -> Matrix {
        Matrix { m11: sx, m12: 0.0, m21: 0.0, m22: sy, dx: 0.0, dy: 0.0 }
    }

    pub fn apply(&self, p: Point) -> Point {
        pt(p.x * self.m11 + p.y * self.m21 + self.dx, p.x * self.m12 + p.y * self.m22 + self.dy)
    }

    /// `self` then `then`.
    pub fn then(&self, then: &Matrix) -> Matrix {
        let (a, b) = (self, then);
        Matrix {
            m11: a.m11 * b.m11 + a.m12 * b.m21,
            m12: a.m11 * b.m12 + a.m12 * b.m22,
            m21: a.m21 * b.m11 + a.m22 * b.m21,
            m22: a.m21 * b.m12 + a.m22 * b.m22,
            dx: a.dx * b.m11 + a.dy * b.m21 + b.dx,
            dy: a.dx * b.m12 + a.dy * b.m22 + b.dy,
        }
    }

    pub fn determinant(&self) -> f32 {
        self.m11 * self.m22 - self.m12 * self.m21
    }

    /// The inverse (None when singular). Each entry is a quotient by the
    /// determinant, so exact inputs give exact (signed-zero) results as
    /// Windows' D2D1InvertMatrix does (Wine's test_invert_matrix).
    pub fn inverse(&self) -> Option<Matrix> {
        let d = self.determinant();
        if d == 0.0 || !d.is_finite() {
            return None;
        }
        let m = self;
        Some(Matrix {
            m11: m.m22 / d,
            m12: -m.m12 / d,
            m21: -m.m21 / d,
            m22: m.m11 / d,
            dx: (m.m21 * m.dy - m.m22 * m.dx) / d,
            dy: (m.m12 * m.dx - m.m11 * m.dy) / d,
        })
    }

    /// How far one unit stretches at most (for flattening tolerances and
    /// stroke widths).
    pub fn max_scale(&self) -> f32 {
        let a = libm::sqrtf(self.m11 * self.m11 + self.m12 * self.m12);
        let b = libm::sqrtf(self.m21 * self.m21 + self.m22 * self.m22);
        a.max(b)
    }
}

/// D2D1_FACTORY_TYPE_MULTI_THREADED's factory or the single-threaded one.
const FACTORY_SINGLE_THREADED: u32 = 0;
const FACTORY_MULTI_THREADED: u32 = 1;

/// D2D1CreateFactory(type, riid, options, out): a new factory (each call
/// its own object), for ID2D1Factory or ID2D1Multithread; a type past
/// MULTI_THREADED is E_INVALIDARG (Wine's test_mt_factory).
#[unsafe(no_mangle)]
pub extern "system" fn D2D1CreateFactory(
    kind: u32,
    riid: *const Guid,
    _options: *const u32,
    out: *mut Obj,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if kind != FACTORY_SINGLE_THREADED && kind != FACTORY_MULTI_THREADED {
        return E_INVALIDARG;
    }
    let Some(f) = factory::new_factory(kind == FACTORY_MULTI_THREADED) else {
        return E_OUTOFMEMORY;
    };
    let r = factory::factory_query(f, riid, out);
    factory::factory_release(f);
    r
}

/// D2D1MakeRotateMatrix(angle in degrees, centre, out): clockwise in
/// Direct2D's y-down space.
#[unsafe(no_mangle)]
pub extern "system" fn D2D1MakeRotateMatrix(angle: f32, center: Point, out: *mut Matrix) {
    let r = angle * (core::f32::consts::PI / 180.0);
    let (s, c) = (libm::sinf(r), libm::cosf(r));
    put(
        out,
        Matrix {
            m11: c,
            m12: s,
            m21: -s,
            m22: c,
            dx: center.x - center.x * c + center.y * s,
            dy: center.y - center.x * s - center.y * c,
        },
    );
}

/// D2D1MakeSkewMatrix(x angle, y angle, centre, out): x' = x + y tan(ax),
/// y' = y + x tan(ay), about the centre (Wine's test_skew_matrix's
/// values, measured on Windows).
#[unsafe(no_mangle)]
pub extern "system" fn D2D1MakeSkewMatrix(ax: f32, ay: f32, center: Point, out: *mut Matrix) {
    let k = core::f32::consts::PI / 180.0;
    let (tx, ty) = (libm::tanf(ax * k), libm::tanf(ay * k));
    put(
        out,
        Matrix { m11: 1.0, m12: ty, m21: tx, m22: 1.0, dx: -center.y * tx, dy: -center.x * ty },
    );
}

/// D2D1IsMatrixInvertible.
#[unsafe(no_mangle)]
pub extern "system" fn D2D1IsMatrixInvertible(m: *const Matrix) -> i32 {
    read(m).and_then(|m| m.inverse()).is_some() as i32
}

/// D2D1InvertMatrix: the inverse in place, TRUE; a singular matrix is
/// left as it is, FALSE.
#[unsafe(no_mangle)]
pub extern "system" fn D2D1InvertMatrix(m: *mut Matrix) -> i32 {
    match read(m).and_then(|v| v.inverse()) {
        Some(inv) => {
            put(m, inv);
            1
        }
        None => 0,
    }
}

/// D2D1SinCos(angle in radians, sin, cos).
#[unsafe(no_mangle)]
pub extern "system" fn D2D1SinCos(angle: f32, s: *mut f32, c: *mut f32) {
    put(s, libm::sinf(angle));
    put(c, libm::cosf(angle));
}

/// D2D1Tan(angle in radians).
#[unsafe(no_mangle)]
pub extern "system" fn D2D1Tan(angle: f32) -> f32 {
    libm::tanf(angle)
}

/// D2D1Vec3Length.
#[unsafe(no_mangle)]
pub extern "system" fn D2D1Vec3Length(x: f32, y: f32, z: f32) -> f32 {
    libm::sqrtf(x * x + y * y + z * z)
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

/// A panic is a bug: its location goes to the ledger (`d2d1.dll!panic
/// <file>`, the line as detail) and the process ends with
/// STATUS_STACK_BUFFER_OVERRUN (fail-fast), as gala's other DLLs do.
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
