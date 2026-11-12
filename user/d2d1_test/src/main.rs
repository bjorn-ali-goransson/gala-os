//! Direct2D (d2d1.dll, d2d1.h) as Office draws with it (ROADMAP OF): the
//! factory, software render targets over WIC bitmaps, GDI device contexts
//! and windows, brushes, geometries and the drawing calls. Expected values
//! are those of Microsoft's documentation and of Wine's d2d1 conformance
//! tests, which run on Windows (dlls/d2d1/tests/d2d1.c: test_mt_factory,
//! test_desktop_dpi, test_invert_matrix, test_skew_matrix,
//! test_wic_target_format, test_clip, test_path_geometry, test_dc_target,
//! test_hwnd_target, test_color_brush), and of what any rasterizer must
//! give: pixels well inside a shape take its colour, pixels well outside
//! keep theirs, aliased edges on pixel centres.
//!
//! - Exports: Office imports d2d1 by ordinal; #1 to #5 are
//!   D2D1CreateFactory, D2D1MakeRotateMatrix, D2D1MakeSkewMatrix,
//!   D2D1IsMatrixInvertible and D2D1InvertMatrix.
//! - The factory: a type past MULTI_THREADED is E_INVALIDARG; both kinds
//!   answer ID2D1Multithread, protected only when multi-threaded; the
//!   desktop DPI is positive.
//! - Matrices: Wine's inversion table (exact, signed zeros), its skew
//!   table, a quarter-turn rotation.
//! - A WIC bitmap target: its formats (the bitmap's; another asked for is
//!   E_INVALIDARG), size, DPI rules (SetDpi), transform and antialias
//!   defaults; Clear, aliased and antialiased FillRectangle, transforms,
//!   clips (Clear inside one too), DrawLine and DrawRectangle widths,
//!   FillEllipse, FillRoundedRectangle, linear and radial gradients, a
//!   bitmap brush that wraps, DrawBitmap (scaled, nearest neighbour),
//!   CreateBitmapFromWicBitmap; EndDraw without BeginDraw
//!   D2DERR_WRONG_STATE.
//! - Brushes: a solid brush's colour, opacity 1 and identity transform.
//! - Geometries: a path geometry's states (counts D2DERR_WRONG_STATE until
//!   Close, a second Close D2DERR_WRONG_STATE), its figure and segment
//!   counts (a closed figure's closing line counted), the fill modes
//!   (FillContainsPoint and pixels), Béziers, an ellipse's bounds, a
//!   rectangle geometry's GetRect.
//! - A DC target: the unsupported formats, size 0 before BindDC, EndDraw
//!   without a DC D2DERR_WRONG_STATE, BindDC's argument errors (keeping
//!   the old binding), pixels through GetPixel, content kept across an
//!   empty frame and around a partial fill.
//! - A window target: its formats, a window it is not E_INVALIDARG,
//!   hardware asked for (served on Windows by its adapter, on gala in
//!   software), Resize, GetHwnd, a frame drawn.
//!
//! What Windows does that the documentation leaves open is printed as
//! `info` lines: drawing outside BeginDraw, CreateBitmapFromWicBitmap of
//! a 32bppBGRA (straight) bitmap, and antialiased edge values.
//!
//! `d2d1_test window` opens a window drawn with Direct2D (a window
//! target) and prints `d2d1_test: window drawn` after its first frame;
//! it ends on the key q (the screen scenario `d2d1_window`).
//!
//! Prints `d2d1_test: ok`, or `d2d1_test: check N got X (line L)` and
//! exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::{null, null_mut};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use ntrt as _;

const NAME: &str = "d2d1_test";

type Handle = usize;
type Hr = i32;
type Guid = [u8; 16];
type Obj = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetModuleHandleA(name: *const u8) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> usize;
    fn GetCommandLineW() -> *const u16;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegisterClassExW(wc: *const WndClassExW) -> u16;
    #[allow(clippy::too_many_arguments)]
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Handle,
        menu: Handle,
        instance: Handle,
        param: *mut c_void,
    ) -> Handle;
    fn DefWindowProcW(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn DestroyWindow(hwnd: Handle) -> i32;
    fn GetMessageW(msg: *mut Msg, hwnd: Handle, min: u32, max: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn PostQuitMessage(code: i32);
    fn BeginPaint(hwnd: Handle, ps: *mut [u8; 72]) -> Handle;
    fn EndPaint(hwnd: Handle, ps: *const [u8; 72]) -> i32;
    fn FillRect(hdc: Handle, rect: *const [i32; 4], brush: Handle) -> i32;
}

#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateCompatibleDC(hdc: Handle) -> Handle;
    fn DeleteDC(hdc: Handle) -> i32;
    fn CreateDIBSection(
        hdc: Handle,
        info: *const u8,
        usage: u32,
        bits: *mut *mut u8,
        section: Handle,
        offset: u32,
    ) -> Handle;
    fn SelectObject(hdc: Handle, object: Handle) -> Handle;
    fn DeleteObject(object: Handle) -> i32;
    fn GetPixel(hdc: Handle, x: i32, y: i32) -> u32;
    fn GetStockObject(index: i32) -> Handle;
}

#[link(name = "windowscodecs", kind = "raw-dylib")]
unsafe extern "system" {
    fn WICCreateImagingFactory_Proxy(version: u32, out: *mut Obj) -> Hr;
}

#[link(name = "d2d1", kind = "raw-dylib")]
unsafe extern "system" {
    fn D2D1CreateFactory(kind: u32, iid: *const Guid, options: *const u32, out: *mut Obj) -> Hr;
    fn D2D1MakeRotateMatrix(angle: f32, center: Point, out: *mut Matrix);
    fn D2D1MakeSkewMatrix(ax: f32, ay: f32, center: Point, out: *mut Matrix);
    fn D2D1IsMatrixInvertible(m: *const Matrix) -> i32;
    fn D2D1InvertMatrix(m: *mut Matrix) -> i32;
}

const S_OK: Hr = 0;
const E_INVALIDARG: Hr = 0x8007_0057_u32 as i32;
const D2DERR_WRONG_STATE: Hr = 0x8899_0001_u32 as i32;
const D2DERR_UNSUPPORTED_PIXEL_FORMAT: Hr = 0x8898_2f80_u32 as i32;

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

const IID_IUNKNOWN: Guid = guid(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_FACTORY: Guid =
    guid(0x0615_2247, 0x6f50, 0x465a, [0x92, 0x45, 0x11, 0x8b, 0xd3, 0xb6, 0x00, 0x7b]);
const IID_MULTITHREAD: Guid =
    guid(0x31e6_e7bc, 0xe0ff, 0x4d46, [0x8c, 0x64, 0xa0, 0xa8, 0xc4, 0x1c, 0x15, 0xd3]);
const IID_RENDER_TARGET: Guid =
    guid(0x2cd9_0694, 0x12e2, 0x11dc, [0x9f, 0xed, 0x00, 0x11, 0x43, 0xa0, 0x55, 0xf9]);

const fn wic_format(last: u8) -> Guid {
    guid(0x6fdd_c324, 0x4e03, 0x4bfe, [0xb1, 0x85, 0x3d, 0x77, 0x76, 0x8d, 0xc9, last])
}
const BGR32: Guid = wic_format(0x0e);
const BGRA32: Guid = wic_format(0x0f);
const PBGRA32: Guid = wic_format(0x10);

const B8G8R8A8: u32 = 87;
const R8G8B8A8: u32 = 28;
const ALPHA_UNKNOWN: u32 = 0;
const ALPHA_PREMULTIPLIED: u32 = 1;
const ALPHA_STRAIGHT: u32 = 2;
const ALPHA_IGNORE: u32 = 3;
const ALIASED: u32 = 1;
const PER_PRIMITIVE: u32 = 0;
const TYPE_DEFAULT: u32 = 0;
const TYPE_HARDWARE: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq)]
struct Point {
    x: f32,
    y: f32,
}

const fn pt(x: f32, y: f32) -> Point {
    Point { x, y }
}

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq)]
struct RectF {
    l: f32,
    t: f32,
    r: f32,
    b: f32,
}

const fn rc(l: f32, t: f32, r: f32, b: f32) -> RectF {
    RectF { l, t, r, b }
}

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq)]
struct ColorF {
    r: f32,
    g: f32,
    b: f32,
    a: f32,
}

const fn color(r: f32, g: f32, b: f32, a: f32) -> ColorF {
    ColorF { r, g, b, a }
}

const RED: ColorF = color(1.0, 0.0, 0.0, 1.0);
const GREEN: ColorF = color(0.0, 1.0, 0.0, 1.0);
const BLUE: ColorF = color(0.0, 0.0, 1.0, 1.0);
const WHITE: ColorF = color(1.0, 1.0, 1.0, 1.0);
const BLACK: ColorF = color(0.0, 0.0, 0.0, 1.0);

#[repr(C)]
#[derive(Clone, Copy, PartialEq)]
struct Matrix {
    m: [f32; 6],
}

const IDENTITY: Matrix = Matrix { m: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0] };

const fn translate(x: f32, y: f32) -> Matrix {
    Matrix { m: [1.0, 0.0, 0.0, 1.0, x, y] }
}

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq)]
struct PixelFormat {
    format: u32,
    alpha: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TargetProps {
    kind: u32,
    format: PixelFormat,
    dpi_x: f32,
    dpi_y: f32,
    usage: u32,
    min_level: u32,
}

const fn props(format: u32, alpha: u32) -> TargetProps {
    TargetProps {
        kind: TYPE_DEFAULT,
        format: PixelFormat { format, alpha },
        dpi_x: 0.0,
        dpi_y: 0.0,
        usage: 0,
        min_level: 0,
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct HwndProps {
    hwnd: Handle,
    w: u32,
    h: u32,
    present: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SizeF {
    w: f32,
    h: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SizeU {
    w: u32,
    h: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Ellipse {
    c: Point,
    rx: f32,
    ry: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RoundedRect {
    r: RectF,
    rx: f32,
    ry: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Stop {
    pos: f32,
    c: ColorF,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct BitmapProps {
    format: PixelFormat,
    dpi_x: f32,
    dpi_y: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Bezier {
    p1: Point,
    p2: Point,
    p3: Point,
}

#[repr(C)]
struct WndClassExW {
    size: u32,
    style: u32,
    proc_: extern "system" fn(Handle, u32, usize, isize) -> isize,
    class_extra: i32,
    window_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
    icon_small: Handle,
}

#[repr(C)]
struct Msg {
    hwnd: Handle,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: [i32; 2],
    private: u32,
}

// ---- Output and checks -------------------------------------------------------------

struct Line {
    buf: [u8; 200],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 200], n: 0 }
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn dec(self, v: u64) -> Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&d[i..])
    }

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 18];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..16 {
            d[2 + i] = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
    }

    fn print(self) {
        let mut written = 0;
        // SAFETY: our buffer; the standard output handle.
        unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                self.buf.as_ptr(),
                self.n as u32,
                &mut written,
                0,
            )
        };
    }
}

fn say(what: &str) {
    Line::new().s(NAME.as_bytes()).s(b": ").s(what.as_bytes()).s(b"\r\n").print();
}

/// An `info` line: what Windows answers, for the record (not checked).
fn info(what: &str, v: u64) {
    Line::new()
        .s(NAME.as_bytes())
        .s(b": info ")
        .s(what.as_bytes())
        .s(b" ")
        .hex(v)
        .s(b"\r\n")
        .print();
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    #[track_caller]
    fn hr(&mut self, got: Hr, want: Hr) -> Result<(), u32> {
        self.ok(got == want, got as u32 as u64)
    }

    /// A float that should be `want` within `eps` (its bits reported).
    #[track_caller]
    fn near(&mut self, got: f32, want: f32, eps: f32) -> Result<(), u32> {
        self.ok((got - want).abs() <= eps, u64::from(got.to_bits()))
    }

    /// A 0xAARRGGBB pixel that should be `want`.
    #[track_caller]
    fn px(&mut self, got: u32, want: u32) -> Result<(), u32> {
        self.ok(got == want, u64::from(got))
    }
}

// ---- COM calls ------------------------------------------------------------------------

/// Method `i` of COM object `o`, as function type `F`.
///
/// # Safety
/// `o` is a live COM object whose vtable has slot `i` of type `F`.
unsafe fn m<F: Copy>(o: Obj, i: usize) -> F {
    // SAFETY: as the caller promises.
    unsafe {
        let vt = *(o as *const *const usize);
        core::mem::transmute_copy::<usize, F>(&*vt.add(i))
    }
}

// SAFETY (every helper below): `o` is a live COM object of the interface
// whose method it calls, with d2d1.h's (or wincodec.h's) slot order.
fn release(o: Obj) -> u32 {
    unsafe { m::<extern "system" fn(Obj) -> u32>(o, 2)(o) }
}

fn qi(o: Obj, iid: &Guid) -> (Hr, Obj) {
    let mut out = null_mut();
    let f: extern "system" fn(Obj, *const Guid, *mut Obj) -> Hr = unsafe { m(o, 0) };
    (f(o, iid, &mut out), out)
}

/// A call returning an object through its only argument.
fn obj0(o: Obj, i: usize) -> (Hr, Obj) {
    let mut out = null_mut();
    let f: extern "system" fn(Obj, *mut Obj) -> Hr = unsafe { m(o, i) };
    (f(o, &mut out), out)
}

/// A call returning an object through its last argument.
fn obj1<A>(o: Obj, i: usize, a: A) -> (Hr, Obj) {
    let mut out = null_mut();
    let f: extern "system" fn(Obj, A, *mut Obj) -> Hr = unsafe { m(o, i) };
    (f(o, a, &mut out), out)
}

fn obj2<A, B>(o: Obj, i: usize, a: A, b: B) -> (Hr, Obj) {
    let mut out = null_mut();
    let f: extern "system" fn(Obj, A, B, *mut Obj) -> Hr = unsafe { m(o, i) };
    (f(o, a, b, &mut out), out)
}

fn obj3<A, B, C>(o: Obj, i: usize, a: A, b: B, c: C) -> (Hr, Obj) {
    let mut out = null_mut();
    let f: extern "system" fn(Obj, A, B, C, *mut Obj) -> Hr = unsafe { m(o, i) };
    (f(o, a, b, c, &mut out), out)
}

fn call0(o: Obj, i: usize) {
    let f: extern "system" fn(Obj) = unsafe { m(o, i) };
    f(o)
}

fn call1<A>(o: Obj, i: usize, a: A) {
    let f: extern "system" fn(Obj, A) = unsafe { m(o, i) };
    f(o, a)
}

fn call2<A, B>(o: Obj, i: usize, a: A, b: B) {
    let f: extern "system" fn(Obj, A, B) = unsafe { m(o, i) };
    f(o, a, b)
}

fn hr0(o: Obj, i: usize) -> Hr {
    let f: extern "system" fn(Obj) -> Hr = unsafe { m(o, i) };
    f(o)
}

fn hr1<A>(o: Obj, i: usize, a: A) -> Hr {
    let f: extern "system" fn(Obj, A) -> Hr = unsafe { m(o, i) };
    f(o, a)
}

fn u32_0(o: Obj, i: usize) -> u32 {
    let f: extern "system" fn(Obj) -> u32 = unsafe { m(o, i) };
    f(o)
}

/// A method returning a structure: through a hidden pointer after `this`
/// (the C++ ABI for methods).
fn ret<T: Default>(o: Obj, i: usize) -> T {
    let mut v = T::default();
    let f: extern "system" fn(Obj, *mut T) -> *mut T = unsafe { m(o, i) };
    f(o, &mut v);
    v
}

// The factory's slots (ID2D1Factory).
const F_GET_DESKTOP_DPI: usize = 4;
const F_CREATE_RECT_GEOMETRY: usize = 5;
const F_CREATE_ELLIPSE_GEOMETRY: usize = 7;
const F_CREATE_PATH_GEOMETRY: usize = 10;
const F_CREATE_WIC_TARGET: usize = 13;
const F_CREATE_HWND_TARGET: usize = 14;
const F_CREATE_DC_TARGET: usize = 16;

// ID2D1RenderTarget's slots.
const RT_CREATE_BITMAP: usize = 4;
const RT_CREATE_BITMAP_FROM_WIC: usize = 5;
const RT_CREATE_BITMAP_BRUSH: usize = 7;
const RT_CREATE_SOLID_BRUSH: usize = 8;
const RT_CREATE_STOPS: usize = 9;
const RT_CREATE_LINEAR: usize = 10;
const RT_CREATE_RADIAL: usize = 11;
const RT_DRAW_LINE: usize = 15;
const RT_DRAW_RECTANGLE: usize = 16;
const RT_FILL_RECTANGLE: usize = 17;
const RT_FILL_ROUNDED: usize = 19;
const RT_DRAW_ELLIPSE: usize = 20;
const RT_FILL_ELLIPSE: usize = 21;
const RT_DRAW_GEOMETRY: usize = 22;
const RT_FILL_GEOMETRY: usize = 23;
const RT_DRAW_BITMAP: usize = 26;
const RT_SET_TRANSFORM: usize = 30;
const RT_GET_TRANSFORM: usize = 31;
const RT_SET_AA: usize = 32;
const RT_GET_AA: usize = 33;
const RT_PUSH_CLIP: usize = 45;
const RT_POP_CLIP: usize = 46;
const RT_CLEAR: usize = 47;
const RT_BEGIN: usize = 48;
const RT_END: usize = 49;
const RT_GET_PIXEL_FORMAT: usize = 50;
const RT_SET_DPI: usize = 51;
const RT_GET_DPI: usize = 52;
const RT_GET_SIZE: usize = 53;
const RT_GET_PIXEL_SIZE: usize = 54;
const HWND_RESIZE: usize = 58;
const HWND_GET_HWND: usize = 59;
const DC_BIND: usize = 57;

fn end_draw(rt: Obj) -> Hr {
    let f: extern "system" fn(Obj, *mut u64, *mut u64) -> Hr = unsafe { m(rt, RT_END) };
    f(rt, null_mut(), null_mut())
}

fn dpi(rt: Obj) -> (f32, f32) {
    let (mut x, mut y) = (0.0f32, 0.0f32);
    call2(rt, RT_GET_DPI, &mut x as *mut f32, &mut y as *mut f32);
    (x, y)
}

fn solid(rt: Obj, c: ColorF) -> Obj {
    obj2(rt, RT_CREATE_SOLID_BRUSH, &c as *const ColorF, null::<u8>()).1
}

fn draw_line(rt: Obj, a: Point, b: Point, brush: Obj, width: f32) {
    let f: extern "system" fn(Obj, Point, Point, Obj, f32, Obj) = unsafe { m(rt, RT_DRAW_LINE) };
    f(rt, a, b, brush, width, null_mut())
}

fn fill_geometry(rt: Obj, g: Obj, brush: Obj) {
    let f: extern "system" fn(Obj, Obj, Obj, Obj) = unsafe { m(rt, RT_FILL_GEOMETRY) };
    f(rt, g, brush, null_mut())
}

fn fill_rect(rt: Obj, r: RectF, brush: Obj) {
    call2(rt, RT_FILL_RECTANGLE, &r as *const RectF, brush);
}

fn clear(rt: Obj, c: ColorF) {
    call1(rt, RT_CLEAR, &c as *const ColorF);
}

// ---- WIC bitmaps and their pixels -----------------------------------------------------

/// IWICImagingFactory::CreateBitmap (cache on load).
fn wic_bitmap(wic: Obj, w: u32, h: u32, format: &Guid) -> Obj {
    let mut out = null_mut();
    let f: extern "system" fn(Obj, u32, u32, *const Guid, u32, *mut Obj) -> Hr =
        unsafe { m(wic, 17) };
    f(wic, w, h, format, 2, &mut out);
    out
}

/// IWICImagingFactory::CreateBitmapFromMemory.
fn wic_from_memory(wic: Obj, w: u32, h: u32, format: &Guid, data: &[u8]) -> Obj {
    let mut out = null_mut();
    let f: extern "system" fn(Obj, u32, u32, *const Guid, u32, u32, *const u8, *mut Obj) -> Hr =
        unsafe { m(wic, 20) };
    f(wic, w, h, format, w * 4, data.len() as u32, data.as_ptr(), &mut out);
    out
}

/// Pixel (x, y) of a 32 bpp WIC bitmap `w` wide, as 0xAARRGGBB.
fn wic_pixel(b: Obj, w: u32, x: u32, y: u32) -> u32 {
    let rect = [x as i32, y as i32, 1, 1];
    let mut px = [0u8; 4];
    let f: extern "system" fn(Obj, *const [i32; 4], u32, u32, *mut u8) -> Hr = unsafe { m(b, 7) };
    f(b, &rect, w * 4, 4, px.as_mut_ptr());
    u32::from_le_bytes(px)
}

/// A WIC target over a new `w` x `h` PBGRA bitmap, drawing begun and
/// cleared to white.
struct Canvas {
    bitmap: Obj,
    rt: Obj,
    w: u32,
}

impl Canvas {
    fn new(f: Obj, wic: Obj, w: u32, h: u32, aa: u32) -> Canvas {
        let bitmap = wic_bitmap(wic, w, h, &PBGRA32);
        let p = props(0, 0);
        let rt = obj2(f, F_CREATE_WIC_TARGET, bitmap, &p as *const TargetProps).1;
        call1(rt, RT_SET_AA, aa);
        call0(rt, RT_BEGIN);
        clear(rt, WHITE);
        Canvas { bitmap, rt, w }
    }

    /// Ends drawing (its result) and keeps the pixels for `at`.
    fn end(&self) -> Hr {
        end_draw(self.rt)
    }

    fn at(&self, x: u32, y: u32) -> u32 {
        wic_pixel(self.bitmap, self.w, x, y)
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        release(self.rt);
        release(self.bitmap);
    }
}

fn channel(px: u32, shift: u32) -> u32 {
    (px >> shift) & 0xff
}

fn section(name: &str) {
    Line::new().s(NAME.as_bytes()).s(b": ").s(name.as_bytes()).s(b"\r\n").print();
}

// ---- The sections ------------------------------------------------------------------------

fn exports(c: &mut Checks) -> Result<(), u32> {
    section("exports");
    // SAFETY: NUL-terminated names; the module is loaded (imported).
    unsafe {
        let d = GetModuleHandleA(c"d2d1.dll".as_ptr().cast());
        c.ok(d != 0, 0)?;
        for (ordinal, name) in [
            (1usize, c"D2D1CreateFactory"),
            (2, c"D2D1MakeRotateMatrix"),
            (3, c"D2D1MakeSkewMatrix"),
            (4, c"D2D1IsMatrixInvertible"),
            (5, c"D2D1InvertMatrix"),
        ] {
            let by_name = GetProcAddress(d, name.as_ptr().cast());
            c.ok(by_name != 0, ordinal as u64)?;
            c.eq(GetProcAddress(d, core::ptr::without_provenance(ordinal)) as u64, by_name as u64)?;
        }
    }
    Ok(())
}

fn factory(c: &mut Checks) -> Result<Obj, u32> {
    section("factory");
    let mut f = null_mut();
    // SAFETY: valid arguments.
    unsafe {
        c.hr(D2D1CreateFactory(2, &IID_FACTORY, null(), &mut f), E_INVALIDARG)?;
        c.ok(f.is_null(), f as u64)?;
        c.hr(D2D1CreateFactory(0, &IID_FACTORY, null(), &mut f), S_OK)?;
    }
    let (r, mt) = qi(f, &IID_MULTITHREAD);
    c.hr(r, S_OK)?;
    c.eq(u64::from(u32_0(mt, 3)), 0)?; // GetMultithreadProtected
    let (r, unk) = qi(mt, &IID_IUNKNOWN);
    c.hr(r, S_OK)?;
    c.eq(unk as u64, f as u64)?;
    release(unk);
    release(mt);
    release(f);
    let mut f = null_mut();
    // SAFETY: valid arguments.
    unsafe { c.hr(D2D1CreateFactory(1, &IID_FACTORY, null(), &mut f), S_OK)? };
    let (r, mt) = qi(f, &IID_MULTITHREAD);
    c.hr(r, S_OK)?;
    c.eq(u64::from(u32_0(mt, 3)), 1)?;
    call0(mt, 4); // Enter
    call0(mt, 5); // Leave
    release(mt);
    let (mut x, mut y) = (0.0f32, 0.0f32);
    call2(f, F_GET_DESKTOP_DPI, &mut x as *mut f32, &mut y as *mut f32);
    c.ok(x > 0.0 && y > 0.0, u64::from(x.to_bits()))?;
    Ok(f)
}

fn matrices(c: &mut Checks) -> Result<(), u32> {
    section("matrices");
    // Wine's test_invert_matrix: exact results, signed zeros included.
    let table: [([f32; 6], [f32; 6], i32); 5] = [
        ([0.0; 6], [0.0; 6], 0),
        ([1.0, 2.0, 1.0, 2.0, 4.0, 8.0], [1.0, 2.0, 1.0, 2.0, 4.0, 8.0], 0),
        ([2.0, 0.0, 0.0, 2.0, 4.0, 8.0], [0.5, -0.0, -0.0, 0.5, -2.0, -4.0], 1),
        ([2.0, 1.0, 2.0, 2.0, 4.0, 8.0], [1.0, -0.5, -1.0, 1.0, 4.0, -6.0], 1),
        ([2.0, 1.0, 3.0, 1.0, 4.0, 8.0], [-1.0, 1.0, 3.0, -2.0, -20.0, 12.0], 1),
    ];
    for (input, want, invertible) in table {
        let mut mx = Matrix { m: input };
        // SAFETY: our matrices.
        unsafe {
            c.eq(D2D1InvertMatrix(&mut mx) as u64, invertible as u64)?;
            c.eq(D2D1IsMatrixInvertible(&Matrix { m: input }) as u64, invertible as u64)?;
        }
        for (got, want) in mx.m.iter().zip(want) {
            c.eq(u64::from(got.to_bits()), u64::from(want.to_bits()))?;
        }
    }
    // Wine's test_skew_matrix (within a few ulps).
    let skews: [(f32, f32, Point, [f32; 6]); 3] = [
        (45.0, 0.0, pt(0.0, 0.0), [1.0, 0.0, 1.0, 1.0, 0.0, 0.0]),
        (-45.0, 45.0, pt(0.1, 0.5), [1.0, 1.0, -1.0, 1.0, 0.5, -0.1]),
        (
            30.0,
            -60.0,
            pt(12.0, -5.0),
            [1.0, -1.732_050_9, 0.577_350_26, 1.0, 2.886_751_2, 20.784_61],
        ),
    ];
    for (ax, ay, center, want) in skews {
        let mut mx = Matrix { m: [0.0; 6] };
        // SAFETY: our matrix.
        unsafe { D2D1MakeSkewMatrix(ax, ay, center, &mut mx) };
        for (&got, want) in mx.m.iter().zip(want) {
            c.near(got, want, 1e-5 * want.abs().max(1.0))?;
        }
    }
    // A quarter turn about (10, 10): clockwise on the y-down screen.
    let mut mx = Matrix { m: [0.0; 6] };
    // SAFETY: our matrix.
    unsafe { D2D1MakeRotateMatrix(90.0, pt(10.0, 10.0), &mut mx) };
    let want = [0.0, 1.0, -1.0, 0.0, 20.0, 0.0];
    for (&got, want) in mx.m.iter().zip(want) {
        c.near(got, want, 1e-5)?;
    }
    Ok(())
}

/// The WIC target's properties and formats (test_wic_target_format).
fn wic_target(c: &mut Checks, f: Obj, wic: Obj) -> Result<(), u32> {
    section("WIC target");
    let cases: [(u32, u32, &Guid, Hr); 8] = [
        (0, ALPHA_PREMULTIPLIED, &PBGRA32, S_OK),
        (0, ALPHA_IGNORE, &BGR32, S_OK),
        (0, ALPHA_UNKNOWN, &PBGRA32, S_OK),
        (B8G8R8A8, ALPHA_UNKNOWN, &BGR32, S_OK),
        (R8G8B8A8, ALPHA_PREMULTIPLIED, &PBGRA32, E_INVALIDARG),
        (R8G8B8A8, ALPHA_UNKNOWN, &BGR32, E_INVALIDARG),
        (B8G8R8A8, ALPHA_PREMULTIPLIED, &BGR32, E_INVALIDARG),
        (B8G8R8A8, ALPHA_IGNORE, &PBGRA32, E_INVALIDARG),
    ];
    for (format, alpha, wic_format, want) in cases {
        let b = wic_bitmap(wic, 24, 16, wic_format);
        c.ok(!b.is_null(), 0)?;
        let mut p = props(format, alpha);
        p.dpi_x = 96.0;
        p.dpi_y = 96.0;
        let (r, rt) = obj2(f, F_CREATE_WIC_TARGET, b, &p as *const TargetProps);
        c.hr(r, want)?;
        if r == S_OK {
            let pf: PixelFormat = ret(rt, RT_GET_PIXEL_FORMAT);
            c.eq(pf.format.into(), B8G8R8A8.into())?;
            let want_alpha = if *wic_format == BGR32 { ALPHA_IGNORE } else { ALPHA_PREMULTIPLIED };
            c.eq(pf.alpha.into(), want_alpha.into())?;
            let size: SizeU = ret(rt, RT_GET_PIXEL_SIZE);
            c.eq(u64::from(size.w) << 32 | u64::from(size.h), 24 << 32 | 16)?;
            release(rt);
        }
        release(b);
    }
    // The defaults, DPI and size.
    let b = wic_bitmap(wic, 32, 16, &PBGRA32);
    let p = props(0, 0);
    let (r, rt) = obj2(f, F_CREATE_WIC_TARGET, b, &p as *const TargetProps);
    c.hr(r, S_OK)?;
    let (r, rt2) = qi(rt, &IID_RENDER_TARGET);
    c.hr(r, S_OK)?;
    c.eq(rt2 as u64, rt as u64)?;
    release(rt2);
    let d = dpi(rt);
    c.ok(d == (96.0, 96.0), u64::from(d.0.to_bits()))?;
    let mut mx = Matrix { m: [9.0; 6] };
    call1(rt, RT_GET_TRANSFORM, &mut mx as *mut Matrix);
    c.ok(mx == IDENTITY, 0)?;
    c.eq(u64::from(u32_0(rt, RT_GET_AA)), u64::from(PER_PRIMITIVE))?;
    // SetDpi (test_clip): one 0 or a negative value is ignored, 0, 0 is
    // 96; the size in DIPs follows, the pixel size does not.
    call2(rt, RT_SET_DPI, 48.0f32, 192.0f32);
    let s: SizeF = ret(rt, RT_GET_SIZE);
    c.ok(s.w == 64.0 && s.h == 8.0, u64::from(s.w.to_bits()))?;
    call2(rt, RT_SET_DPI, 0.0f32, 96.0f32);
    c.ok(dpi(rt) == (48.0, 192.0), 0)?;
    call2(rt, RT_SET_DPI, -10.0f32, 96.0f32);
    c.ok(dpi(rt) == (48.0, 192.0), 0)?;
    call2(rt, RT_SET_DPI, 0.0f32, 0.0f32);
    c.ok(dpi(rt) == (96.0, 96.0), 0)?;
    // EndDraw without BeginDraw.
    c.hr(end_draw(rt), D2DERR_WRONG_STATE)?;
    // Drawing outside BeginDraw: how the next frame ends.
    let brush = solid(rt, RED);
    fill_rect(rt, rc(0.0, 0.0, 4.0, 4.0), brush);
    call0(rt, RT_BEGIN);
    info("EndDraw after a draw outside BeginDraw", end_draw(rt) as u32 as u64);
    call0(rt, RT_BEGIN);
    c.hr(end_draw(rt), S_OK)?;
    release(brush);
    release(rt);
    release(b);
    Ok(())
}

/// Shapes, transforms and clips on a WIC target.
fn drawing(c: &mut Checks, f: Obj, wic: Obj) -> Result<(), u32> {
    section("drawing");
    let cv = Canvas::new(f, wic, 32, 32, ALIASED);
    let rt = cv.rt;
    let red = solid(rt, RED);
    let black = solid(rt, BLACK);
    fill_rect(rt, rc(2.0, 2.0, 10.0, 10.0), red);
    // Through a transform.
    let t = translate(16.0, 0.0);
    call1(rt, RT_SET_TRANSFORM, &t as *const Matrix);
    fill_rect(rt, rc(0.0, 2.0, 4.0, 6.0), black);
    call1(rt, RT_SET_TRANSFORM, &IDENTITY as *const Matrix);
    // A clip: only its part of the rectangle.
    let clip = rc(24.0, 0.0, 28.0, 4.0);
    call2(rt, RT_PUSH_CLIP, &clip as *const RectF, ALIASED);
    fill_rect(rt, rc(22.0, 0.0, 32.0, 8.0), red);
    call0(rt, RT_POP_CLIP);
    // Clear inside a clip.
    let clip = rc(28.0, 28.0, 32.0, 32.0);
    call2(rt, RT_PUSH_CLIP, &clip as *const RectF, ALIASED);
    clear(rt, BLACK);
    call0(rt, RT_POP_CLIP);
    // A line 2 wide along y = 20 (rows 19 and 20), flat caps.
    draw_line(rt, pt(2.0, 20.0), pt(14.0, 20.0), black, 2.0);
    // A rectangle outline 1 wide on half-pixel edges.
    let r = rc(2.5, 24.5, 9.5, 29.5);
    let f2: extern "system" fn(Obj, *const RectF, Obj, f32, Obj) =
        unsafe { m(rt, RT_DRAW_RECTANGLE) };
    f2(rt, &r, black, 1.0, null_mut());
    // An ellipse.
    let e = Ellipse { c: pt(20.0, 24.0), rx: 4.0, ry: 4.0 };
    call2(rt, RT_FILL_ELLIPSE, &e as *const Ellipse, red);
    c.hr(cv.end(), S_OK)?;
    let (r_, w_, k_) = (0xffff_0000, 0xffff_ffff, 0xff00_0000);
    c.px(cv.at(1, 1), w_)?;
    c.px(cv.at(2, 2), r_)?;
    c.px(cv.at(9, 9), r_)?;
    c.px(cv.at(10, 9), w_)?;
    c.px(cv.at(16, 2), k_)?;
    c.px(cv.at(19, 5), k_)?;
    c.px(cv.at(20, 2), w_)?;
    c.px(cv.at(15, 2), w_)?;
    c.px(cv.at(24, 0), r_)?;
    c.px(cv.at(27, 3), r_)?;
    c.px(cv.at(23, 1), w_)?;
    c.px(cv.at(28, 1), w_)?;
    c.px(cv.at(25, 5), w_)?;
    c.px(cv.at(28, 28), k_)?;
    c.px(cv.at(31, 31), k_)?;
    c.px(cv.at(27, 31), w_)?;
    c.px(cv.at(5, 19), k_)?;
    c.px(cv.at(5, 20), k_)?;
    c.px(cv.at(5, 18), w_)?;
    c.px(cv.at(5, 21), w_)?;
    c.px(cv.at(1, 20), w_)?;
    c.px(cv.at(14, 20), w_)?;
    c.px(cv.at(5, 24), k_)?;
    c.px(cv.at(2, 26), k_)?;
    c.px(cv.at(9, 26), k_)?;
    c.px(cv.at(5, 29), k_)?;
    c.px(cv.at(5, 26), w_)?;
    c.px(cv.at(5, 30), w_)?;
    c.px(cv.at(20, 24), r_)?;
    c.px(cv.at(17, 21), r_)?; // centre (17.5, 21.5): inside the circle
    c.px(cv.at(16, 20), w_)?; // centre (16.5, 20.5): outside
    release(red);
    release(black);
    drop(cv);

    // Antialiased: a half-covered column, a rounded rectangle, opacity.
    let cv = Canvas::new(f, wic, 16, 16, PER_PRIMITIVE);
    let rt = cv.rt;
    let blue = solid(rt, BLUE);
    fill_rect(rt, rc(2.5, 2.0, 4.5, 6.0), blue);
    let rr = RoundedRect { r: rc(6.0, 2.0, 14.0, 10.0), rx: 3.0, ry: 3.0 };
    call2(rt, RT_FILL_ROUNDED, &rr as *const RoundedRect, blue);
    let half_red = solid(rt, color(1.0, 0.0, 0.0, 0.5));
    fill_rect(rt, rc(0.0, 12.0, 16.0, 16.0), half_red);
    c.hr(cv.end(), S_OK)?;
    let px = cv.at(3, 3);
    c.px(px, 0xff00_00ff)?;
    // Half the pixel blue over white: blue full, red and green half.
    let px = cv.at(2, 3);
    info("half-covered pixel", px.into());
    c.ok(channel(px, 0) == 255 && (112..=144).contains(&channel(px, 16)), px.into())?;
    c.px(cv.at(10, 6), 0xff00_00ff)?;
    // The rounded corner leaves the rectangle's corner pixel mostly white.
    c.ok(channel(cv.at(6, 2), 16) > 100, cv.at(6, 2).into())?;
    // Red at half opacity over white: red full, green and blue half.
    let px = cv.at(8, 14);
    c.ok(
        channel(px, 16) == 255 && (126..=129).contains(&channel(px, 8)) && channel(px, 24) == 255,
        px.into(),
    )?;
    release(blue);
    release(half_red);
    Ok(())
}

/// Gradients, a bitmap brush and DrawBitmap.
fn paints(c: &mut Checks, f: Obj, wic: Obj) -> Result<(), u32> {
    section("paints");
    let cv = Canvas::new(f, wic, 32, 32, ALIASED);
    let rt = cv.rt;
    let stops = [Stop { pos: 0.0, c: RED }, Stop { pos: 1.0, c: BLUE }];
    let mut coll = null_mut();
    let fs: extern "system" fn(Obj, *const Stop, u32, u32, u32, *mut Obj) -> Hr =
        unsafe { m(rt, RT_CREATE_STOPS) };
    c.hr(fs(rt, stops.as_ptr(), 2, 0, 0, &mut coll), S_OK)?;
    c.eq(u32_0(coll, 4).into(), 2)?; // GetGradientStopCount
    let line = [pt(0.0, 0.0), pt(32.0, 0.0)];
    let (r, linear) = obj3(rt, RT_CREATE_LINEAR, &line as *const [Point; 2], null::<u8>(), coll);
    c.hr(r, S_OK)?;
    fill_rect(rt, rc(0.0, 0.0, 32.0, 4.0), linear);
    let radial_props = [pt(16.0, 16.0), pt(0.0, 0.0), pt(8.0, 8.0)];
    let white_black = [Stop { pos: 0.0, c: WHITE }, Stop { pos: 1.0, c: BLACK }];
    let mut coll2 = null_mut();
    c.hr(fs(rt, white_black.as_ptr(), 2, 0, 0, &mut coll2), S_OK)?;
    let (r, radial) =
        obj3(rt, RT_CREATE_RADIAL, &radial_props as *const [Point; 3], null::<u8>(), coll2);
    c.hr(r, S_OK)?;
    fill_rect(rt, rc(4.0, 4.0, 28.0, 28.0), radial);
    c.hr(cv.end(), S_OK)?;
    let left = cv.at(0, 1);
    let right = cv.at(31, 1);
    let mid = cv.at(16, 1);
    c.ok(channel(left, 16) >= 240 && channel(left, 0) <= 15, left.into())?;
    c.ok(channel(right, 0) >= 240 && channel(right, 16) <= 15, right.into())?;
    let sum = channel(mid, 16) + channel(mid, 0);
    c.ok((100..=156).contains(&channel(mid, 16)) && (245..=265).contains(&sum), mid.into())?;
    info("linear gradient's middle", mid.into());
    // The pixel at the centre: its own centre 0.7 from the gradient's (t
    // about 0.09).
    let centre = cv.at(16, 16);
    c.ok(channel(centre, 8) >= 220, centre.into())?;
    let edge = cv.at(16, 26); // 10.5 from the centre: past the radius
    c.px(edge, 0xff00_0000)?;
    let between = cv.at(20, 16); // 4.5 from the centre: grey
    c.ok((90..=170).contains(&channel(between, 8)), between.into())?;
    release(linear);
    release(radial);
    release(coll);
    release(coll2);
    drop(cv);

    // A 2 x 2 bitmap: red, green / blue, white.
    let cv = Canvas::new(f, wic, 32, 32, ALIASED);
    let rt = cv.rt;
    let pixels: [u8; 16] = [0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255];
    let bp = BitmapProps {
        format: PixelFormat { format: B8G8R8A8, alpha: ALPHA_PREMULTIPLIED },
        dpi_x: 96.0,
        dpi_y: 96.0,
    };
    let mut bitmap = null_mut();
    let fb: extern "system" fn(Obj, SizeU, *const u8, u32, *const BitmapProps, *mut Obj) -> Hr =
        unsafe { m(rt, RT_CREATE_BITMAP) };
    c.hr(fb(rt, SizeU { w: 2, h: 2 }, pixels.as_ptr(), 8, &bp, &mut bitmap), S_OK)?;
    let s: SizeU = ret(bitmap, 5);
    c.eq(u64::from(s.w) << 32 | u64::from(s.h), 2 << 32 | 2)?;
    // Wrapped in both directions, nearest neighbour.
    let bbp: [u32; 3] = [1, 1, 0];
    let (r, bb) = obj3(rt, RT_CREATE_BITMAP_BRUSH, bitmap, &bbp as *const [u32; 3], null::<u8>());
    c.hr(r, S_OK)?;
    fill_rect(rt, rc(0.0, 0.0, 8.0, 8.0), bb);
    // DrawBitmap: twice the size at (10, 10).
    let dst = rc(10.0, 10.0, 14.0, 14.0);
    let fd: extern "system" fn(Obj, Obj, *const RectF, f32, u32, *const RectF) =
        unsafe { m(rt, RT_DRAW_BITMAP) };
    fd(rt, bitmap, &dst, 1.0, 0, null());
    // From a WIC bitmap: one 2 x 1 PBGRA (green, half-transparent black).
    let wb = wic_from_memory(wic, 2, 1, &PBGRA32, &[0, 255, 0, 255, 0, 0, 0, 128]);
    let (r, from_wic) = obj2(rt, RT_CREATE_BITMAP_FROM_WIC, wb, null::<u8>());
    c.hr(r, S_OK)?;
    let pf: PixelFormat = ret(from_wic, 6);
    c.eq(
        u64::from(pf.format) << 32 | u64::from(pf.alpha),
        u64::from(B8G8R8A8) << 32 | u64::from(ALPHA_PREMULTIPLIED),
    )?;
    let dst = rc(20.0, 20.0, 22.0, 21.0);
    fd(rt, from_wic, &dst, 1.0, 0, null());
    let straight = wic_from_memory(wic, 1, 1, &BGRA32, &[0, 0, 255, 255]);
    let (r, b2) = obj2(rt, RT_CREATE_BITMAP_FROM_WIC, straight, null::<u8>());
    info("CreateBitmapFromWicBitmap(32bppBGRA)", r as u32 as u64);
    if r == S_OK {
        release(b2);
    }
    c.hr(cv.end(), S_OK)?;
    c.px(cv.at(0, 0), 0xffff_0000)?;
    c.px(cv.at(1, 0), 0xff00_ff00)?;
    c.px(cv.at(2, 0), 0xffff_0000)?;
    c.px(cv.at(1, 1), 0xffff_ffff)?;
    c.px(cv.at(0, 3), 0xff00_00ff)?;
    c.px(cv.at(10, 10), 0xffff_0000)?;
    c.px(cv.at(11, 11), 0xffff_0000)?;
    c.px(cv.at(12, 10), 0xff00_ff00)?;
    c.px(cv.at(10, 12), 0xff00_00ff)?;
    c.px(cv.at(13, 13), 0xffff_ffff)?;
    c.px(cv.at(14, 13), 0xffff_ffff)?;
    c.px(cv.at(20, 20), 0xff00_ff00)?;
    // Premultiplied black at alpha 128 over white: 127.
    let px = cv.at(21, 20);
    c.ok(channel(px, 24) == 255 && (126..=128).contains(&channel(px, 0)), px.into())?;
    release(bb);
    release(bitmap);
    release(from_wic);
    release(wb);
    release(straight);
    Ok(())
}

fn brushes(c: &mut Checks, f: Obj, wic: Obj) -> Result<(), u32> {
    section("brushes");
    let cv = Canvas::new(f, wic, 4, 4, ALIASED);
    let want = color(0.5, 0.25, 0.75, 0.5);
    let b = solid(cv.rt, want);
    let got: ColorF = ret(b, 9);
    c.ok(got == want, u64::from(got.r.to_bits()))?;
    let op: extern "system" fn(Obj) -> f32 = unsafe { m(b, 6) };
    c.ok(op(b) == 1.0, 0)?;
    let mut mx = Matrix { m: [5.0; 6] };
    call1(b, 7, &mut mx as *mut Matrix);
    c.ok(mx == IDENTITY, 0)?;
    // GetFactory: the target's factory.
    let mut fac = null_mut();
    call1(b, 3, &mut fac as *mut Obj);
    c.eq(fac as u64, f as u64)?;
    release(fac);
    release(b);
    c.hr(cv.end(), S_OK)?;
    Ok(())
}

fn path(f: Obj, figs: &[(&[Point], bool)], winding: bool) -> Obj {
    let (_, g) = obj0(f, F_CREATE_PATH_GEOMETRY);
    let (_, sink) = obj0(g, 17);
    call1(sink, 3, u32::from(winding)); // SetFillMode
    for (pts, closed) in figs {
        call2(sink, 5, pts[0], 0u32); // BeginFigure, filled
        call2(sink, 6, pts[1..].as_ptr(), (pts.len() - 1) as u32); // AddLines
        call1(sink, 8, u32::from(*closed)); // EndFigure
    }
    hr0(sink, 9); // Close
    release(sink);
    g
}

fn fill_contains(g: Obj, p: Point) -> (Hr, i32) {
    let mut v = -1;
    let f: extern "system" fn(Obj, Point, *const Matrix, f32, *mut i32) -> Hr = unsafe { m(g, 7) };
    (f(g, p, null(), 0.25, &mut v), v)
}

fn geometries(c: &mut Checks, f: Obj, wic: Obj) -> Result<(), u32> {
    section("geometries");
    // A path geometry's states (test_path_geometry).
    let (r, g) = obj0(f, F_CREATE_PATH_GEOMETRY);
    c.hr(r, S_OK)?;
    let mut n = 99u32;
    c.hr(hr1(g, 20, &mut n as *mut u32), D2DERR_WRONG_STATE)?;
    let (r, sink) = obj0(g, 17);
    c.hr(r, S_OK)?;
    let (r, _) = obj0(g, 17);
    c.hr(r, D2DERR_WRONG_STATE)?;
    c.hr(hr1(g, 19, &mut n as *mut u32), D2DERR_WRONG_STATE)?;
    call2(sink, 5, pt(0.0, 0.0), 0u32);
    call1(sink, 10, pt(16.0, 0.0)); // AddLine
    call1(sink, 10, pt(0.0, 16.0));
    call1(sink, 8, 1u32); // closed
    // A hollow figure is not filled.
    call2(sink, 5, pt(20.0, 20.0), 1u32);
    let bz = Bezier { p1: pt(30.0, 20.0), p2: pt(30.0, 30.0), p3: pt(20.0, 30.0) };
    call1(sink, 11, &bz as *const Bezier); // AddBezier
    call1(sink, 8, 0u32);
    c.hr(hr0(sink, 9), S_OK)?;
    c.hr(hr0(sink, 9), D2DERR_WRONG_STATE)?;
    release(sink);
    c.hr(hr1(g, 20, &mut n as *mut u32), S_OK)?;
    c.eq(n.into(), 2)?;
    c.hr(hr1(g, 19, &mut n as *mut u32), S_OK)?;
    c.eq(n.into(), 4)?; // two lines, the closing line, the Bézier
    c.ok(fill_contains(g, pt(2.0, 2.0)) == (S_OK, 1), 0)?;
    c.ok(fill_contains(g, pt(14.0, 14.0)) == (S_OK, 0), 0)?;
    c.ok(fill_contains(g, pt(24.0, 25.0)) == (S_OK, 0), 0)?;
    let mut bounds = RectF::default();
    let gb: extern "system" fn(Obj, *const Matrix, *mut RectF) -> Hr = unsafe { m(g, 4) };
    c.hr(gb(g, null(), &mut bounds), S_OK)?;
    c.near(bounds.l, 0.0, 0.01)?;
    c.near(bounds.b, 30.0, 0.01)?;
    // The Bézier's farthest point: x = 20 + 0.75 * 10.
    c.near(bounds.r, 27.5, 0.05)?;
    release(g);

    // Nested squares, both clockwise: alternate leaves a hole, winding not.
    let outer = [pt(0.0, 0.0), pt(16.0, 0.0), pt(16.0, 16.0), pt(0.0, 16.0)];
    let inner = [pt(4.0, 4.0), pt(12.0, 4.0), pt(12.0, 12.0), pt(4.0, 12.0)];
    let alternate = path(f, &[(&outer, true), (&inner, true)], false);
    let winding = path(f, &[(&outer, true), (&inner, true)], true);
    c.ok(fill_contains(alternate, pt(8.0, 8.0)) == (S_OK, 0), 0)?;
    c.ok(fill_contains(alternate, pt(2.0, 8.0)) == (S_OK, 1), 0)?;
    c.ok(fill_contains(winding, pt(8.0, 8.0)) == (S_OK, 1), 0)?;
    let cv = Canvas::new(f, wic, 40, 16, ALIASED);
    let black = solid(cv.rt, BLACK);
    fill_geometry(cv.rt, alternate, black);
    let t = translate(20.0, 0.0);
    call1(cv.rt, RT_SET_TRANSFORM, &t as *const Matrix);
    fill_geometry(cv.rt, winding, black);
    c.hr(cv.end(), S_OK)?;
    c.px(cv.at(2, 8), 0xff00_0000)?;
    c.px(cv.at(8, 8), 0xffff_ffff)?;
    c.px(cv.at(28, 8), 0xff00_0000)?;
    c.px(cv.at(17, 8), 0xffff_ffff)?;
    release(alternate);
    release(winding);

    // DrawGeometry and DrawEllipse strokes.
    let cv2 = Canvas::new(f, wic, 32, 32, ALIASED);
    let (_, rg) = obj1(f, F_CREATE_RECT_GEOMETRY, &rc(4.0, 4.0, 12.0, 12.0) as *const RectF);
    let mut got = RectF::default();
    call1(rg, 17, &mut got as *mut RectF); // GetRect
    c.ok(got == rc(4.0, 4.0, 12.0, 12.0), 0)?;
    let fg: extern "system" fn(Obj, Obj, Obj, f32, Obj) = unsafe { m(cv2.rt, RT_DRAW_GEOMETRY) };
    fg(cv2.rt, rg, black, 2.0, null_mut());
    let e = Ellipse { c: pt(22.0, 22.0), rx: 6.0, ry: 6.0 };
    let fe: extern "system" fn(Obj, *const Ellipse, Obj, f32, Obj) =
        unsafe { m(cv2.rt, RT_DRAW_ELLIPSE) };
    fe(cv2.rt, &e, black, 2.0, null_mut());
    c.hr(cv2.end(), S_OK)?;
    c.px(cv2.at(3, 8), 0xff00_0000)?;
    c.px(cv2.at(4, 8), 0xff00_0000)?;
    c.px(cv2.at(8, 8), 0xffff_ffff)?;
    c.px(cv2.at(2, 8), 0xffff_ffff)?;
    c.px(cv2.at(3, 3), 0xff00_0000)?; // the miter corner
    c.px(cv2.at(22, 16), 0xff00_0000)?; // the ellipse's top
    c.px(cv2.at(22, 22), 0xffff_ffff)?;
    c.px(cv2.at(22, 13), 0xffff_ffff)?;
    // An ellipse geometry's bounds.
    let ell = Ellipse { c: pt(10.0, 10.0), rx: 5.0, ry: 3.0 };
    let (r, eg) = obj1(f, F_CREATE_ELLIPSE_GEOMETRY, &ell as *const Ellipse);
    c.hr(r, S_OK)?;
    c.hr(gb(eg, null(), &mut bounds), S_OK)?;
    c.ok(bounds == rc(5.0, 7.0, 15.0, 13.0), u64::from(bounds.l.to_bits()))?;
    release(eg);
    release(rg);
    release(black);
    Ok(())
}

/// A DIB section of `w` x `h` selected into a new memory DC.
fn dib_dc(w: i32, h: i32) -> Handle {
    let mut info = [0u8; 44];
    info[0] = 40;
    info[4..8].copy_from_slice(&w.to_le_bytes());
    info[8..12].copy_from_slice(&(-h).to_le_bytes());
    info[12] = 1;
    info[14] = 32;
    // SAFETY: plain GDI calls with our header.
    unsafe {
        let dc = CreateCompatibleDC(0);
        let mut bits = null_mut();
        let bm = CreateDIBSection(dc, info.as_ptr(), 0, &mut bits, 0, 0);
        DeleteObject(SelectObject(dc, bm));
        dc
    }
}

fn dc_target(c: &mut Checks, f: Obj) -> Result<(), u32> {
    section("DC target");
    for (format, alpha) in
        [(0, ALPHA_PREMULTIPLIED), (B8G8R8A8, ALPHA_UNKNOWN), (R8G8B8A8, ALPHA_PREMULTIPLIED)]
    {
        let mut p = props(format, alpha);
        p.dpi_x = 96.0;
        p.dpi_y = 96.0;
        let (r, _) = obj1(f, F_CREATE_DC_TARGET, &p as *const TargetProps);
        c.hr(r, D2DERR_UNSUPPORTED_PIXEL_FORMAT)?;
    }
    let mut p = props(B8G8R8A8, ALPHA_PREMULTIPLIED);
    p.dpi_x = 96.0;
    p.dpi_y = 96.0;
    let (r, rt) = obj1(f, F_CREATE_DC_TARGET, &p as *const TargetProps);
    c.hr(r, S_OK)?;
    let s: SizeU = ret(rt, RT_GET_PIXEL_SIZE);
    c.eq(u64::from(s.w) << 32 | u64::from(s.h), 0)?;
    call0(rt, RT_BEGIN);
    c.hr(end_draw(rt), D2DERR_WRONG_STATE)?;
    let rect = [0i32, 0, 16, 16];
    let bind = |rt: Obj, dc: Handle, r: *const [i32; 4]| -> Hr {
        let f: extern "system" fn(Obj, Handle, *const [i32; 4]) -> Hr = unsafe { m(rt, DC_BIND) };
        f(rt, dc, r)
    };
    c.hr(bind(rt, 0, &rect), E_INVALIDARG)?;
    let dc = dib_dc(16, 16);
    c.hr(bind(rt, dc, &rect), S_OK)?;
    let s: SizeF = ret(rt, RT_GET_SIZE);
    c.ok(s.w == 16.0 && s.h == 16.0, u64::from(s.w.to_bits()))?;
    call0(rt, RT_BEGIN);
    clear(rt, RED);
    c.hr(end_draw(rt), S_OK)?;
    // SAFETY (GetPixel, FillRect below): our DC.
    unsafe { c.eq(GetPixel(dc, 0, 0).into(), 0x0000ff)? };
    // An invalid DC keeps the binding.
    c.hr(bind(rt, 0xdead_beef, &rect), E_INVALIDARG)?;
    call0(rt, RT_BEGIN);
    clear(rt, BLUE);
    c.hr(end_draw(rt), S_OK)?;
    unsafe { c.eq(GetPixel(dc, 0, 0).into(), 0xff0000)? };
    // GDI's grey survives an empty frame, and the half not drawn on.
    unsafe { FillRect(dc, &rect, GetStockObject(2)) }; // GRAY_BRUSH
    unsafe { c.eq(GetPixel(dc, 0, 0).into(), 0x808080)? };
    call0(rt, RT_BEGIN);
    c.hr(end_draw(rt), S_OK)?;
    unsafe { c.eq(GetPixel(dc, 0, 0).into(), 0x808080)? };
    let green = solid(rt, GREEN);
    call0(rt, RT_BEGIN);
    fill_rect(rt, rc(0.0, 0.0, 8.0, 16.0), green);
    c.hr(end_draw(rt), S_OK)?;
    unsafe {
        c.eq(GetPixel(dc, 0, 0).into(), 0x00ff00)?;
        c.eq(GetPixel(dc, 12, 0).into(), 0x808080)?;
    }
    release(green);
    release(rt);
    // SAFETY: our DC.
    unsafe { DeleteDC(dc) };
    Ok(())
}

extern "system" fn wndproc(hwnd: Handle, msg: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: the window's own messages.
    unsafe {
        match msg {
            WM_PAINT if DEMO.load(Ordering::Relaxed) != 0 => {
                let mut ps = [0u8; 72];
                BeginPaint(hwnd, &mut ps);
                draw_demo(DEMO.load(Ordering::Relaxed) as Obj);
                EndPaint(hwnd, &ps);
                if !DRAWN.swap(true, Ordering::Relaxed) {
                    say("window drawn");
                }
                0
            }
            WM_CHAR if wparam == usize::from(b'q') => {
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

const WM_DESTROY: u32 = 0x0002;
const WM_PAINT: u32 = 0x000f;
const WM_CHAR: u32 = 0x0102;
const WS_OVERLAPPEDWINDOW: u32 = 0x00cf_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_POPUP: u32 = 0x8000_0000;

/// The demo window's target, and whether it was drawn once.
static DEMO: AtomicUsize = AtomicUsize::new(0);
static DRAWN: AtomicBool = AtomicBool::new(false);

fn wide<const N: usize>(s: &str) -> [u16; N] {
    let mut w = [0u16; N];
    for (i, b) in s.bytes().enumerate() {
        w[i] = u16::from(b);
    }
    w
}

fn window(style: u32, x: i32, y: i32, w: i32, h: i32, title: &str) -> Handle {
    let class = wide::<16>("GalaD2D");
    let title = wide::<32>(title);
    let wc = WndClassExW {
        size: core::mem::size_of::<WndClassExW>() as u32,
        style: 0,
        proc_: wndproc,
        class_extra: 0,
        window_extra: 0,
        instance: 0,
        icon: 0,
        cursor: 0,
        background: 0,
        menu_name: null(),
        class_name: class.as_ptr(),
        icon_small: 0,
    };
    // SAFETY: valid class and strings (a second registration fails
    // harmlessly).
    unsafe {
        RegisterClassExW(&wc);
        CreateWindowExW(0, class.as_ptr(), title.as_ptr(), style, x, y, w, h, 0, 0, 0, null_mut())
    }
}

fn hwnd_target(c: &mut Checks, f: Obj) -> Result<(), u32> {
    section("window target");
    let hwnd = window(WS_POPUP, 0, 0, 64, 64, "d2d1_test");
    c.ok(hwnd != 0, 0)?;
    let p = props(B8G8R8A8, ALPHA_PREMULTIPLIED);
    let mut hp = HwndProps { hwnd: 0, w: 64, h: 64, present: 0 };
    let (r, _) = obj2(f, F_CREATE_HWND_TARGET, &p as *const TargetProps, &hp as *const HwndProps);
    c.ok(r != S_OK, r as u32 as u64)?;
    hp.hwnd = hwnd;
    let formats: [(u32, u32, Hr, u32); 5] = [
        (0, ALPHA_UNKNOWN, S_OK, ALPHA_IGNORE),
        (0, ALPHA_PREMULTIPLIED, S_OK, ALPHA_PREMULTIPLIED),
        (0, ALPHA_STRAIGHT, D2DERR_UNSUPPORTED_PIXEL_FORMAT, 0),
        (B8G8R8A8, ALPHA_UNKNOWN, S_OK, ALPHA_IGNORE),
        (B8G8R8A8, ALPHA_STRAIGHT, D2DERR_UNSUPPORTED_PIXEL_FORMAT, 0),
    ];
    for (format, alpha, want, want_alpha) in formats {
        let p = props(format, alpha);
        let (r, rt) =
            obj2(f, F_CREATE_HWND_TARGET, &p as *const TargetProps, &hp as *const HwndProps);
        c.hr(r, want)?;
        if r == S_OK {
            let pf: PixelFormat = ret(rt, RT_GET_PIXEL_FORMAT);
            c.eq(
                u64::from(pf.format) << 32 | u64::from(pf.alpha),
                u64::from(B8G8R8A8) << 32 | u64::from(want_alpha),
            )?;
            release(rt);
        }
    }
    // Hardware asked for: Windows' adapter, gala's software rasterizer.
    let mut p = props(0, 0);
    p.kind = TYPE_HARDWARE;
    let (r, rt) = obj2(f, F_CREATE_HWND_TARGET, &p as *const TargetProps, &hp as *const HwndProps);
    c.hr(r, S_OK)?;
    let get_hwnd: extern "system" fn(Obj) -> Handle = unsafe { m(rt, HWND_GET_HWND) };
    c.eq(get_hwnd(rt) as u64, hwnd as u64)?;
    let size = SizeU { w: 128, h: 64 };
    c.hr(hr1(rt, HWND_RESIZE, &size as *const SizeU), S_OK)?;
    let s: SizeU = ret(rt, RT_GET_PIXEL_SIZE);
    c.eq(u64::from(s.w) << 32 | u64::from(s.h), 128 << 32 | 64)?;
    call0(rt, RT_BEGIN);
    clear(rt, GREEN);
    let red = solid(rt, RED);
    fill_rect(rt, rc(8.0, 8.0, 24.0, 24.0), red);
    c.hr(end_draw(rt), S_OK)?;
    release(red);
    release(rt);
    // SAFETY: our window.
    unsafe { DestroyWindow(hwnd) };
    Ok(())
}

/// The demo window's frame: a gradient, shapes, strokes and a path.
fn draw_demo(rt: Obj) {
    call0(rt, RT_BEGIN);
    clear(rt, WHITE);
    let s: SizeF = ret(rt, RT_GET_SIZE);
    let stops = [
        Stop { pos: 0.0, c: color(0.10, 0.30, 0.70, 1.0) },
        Stop { pos: 1.0, c: color(0.55, 0.80, 1.0, 1.0) },
    ];
    let mut coll = null_mut();
    let fs: extern "system" fn(Obj, *const Stop, u32, u32, u32, *mut Obj) -> Hr =
        unsafe { m(rt, RT_CREATE_STOPS) };
    fs(rt, stops.as_ptr(), 2, 0, 0, &mut coll);
    let line = [pt(0.0, 0.0), pt(0.0, s.h)];
    let (_, sky) = obj3(rt, RT_CREATE_LINEAR, &line as *const [Point; 2], null::<u8>(), coll);
    fill_rect(rt, rc(0.0, 0.0, s.w, s.h), sky);
    // A red rounded rectangle, a green circle, a yellow star.
    let red = solid(rt, color(0.86, 0.08, 0.24, 1.0));
    let rr = RoundedRect { r: rc(20.0, 20.0, 140.0, 100.0), rx: 16.0, ry: 16.0 };
    call2(rt, RT_FILL_ROUNDED, &rr as *const RoundedRect, red);
    let green = solid(rt, color(0.13, 0.55, 0.13, 1.0));
    let e = Ellipse { c: pt(220.0, 70.0), rx: 50.0, ry: 50.0 };
    call2(rt, RT_FILL_ELLIPSE, &e as *const Ellipse, green);
    let black = solid(rt, BLACK);
    let fe: extern "system" fn(Obj, *const Ellipse, Obj, f32, Obj) =
        unsafe { m(rt, RT_DRAW_ELLIPSE) };
    fe(rt, &e, black, 4.0, null_mut());
    // Lines of growing width.
    for i in 0..5 {
        let y = 130.0 + i as f32 * 14.0;
        draw_line(rt, pt(20.0, y), pt(140.0, y + 8.0), black, 1.0 + i as f32 * 2.0);
    }
    // A five-pointed star, filled by the winding rule.
    let mut fac = null_mut();
    call1(rt, 3, &mut fac as *mut Obj);
    let mut star = [pt(0.0, 0.0); 5];
    for (i, p) in star.iter_mut().enumerate() {
        let a = (i as f32 * 144.0 - 90.0) * core::f32::consts::PI / 180.0;
        // A polynomial sine/cosine is enough for a picture.
        let (sn, cs) = sincos(a);
        *p = pt(220.0 + 60.0 * cs, 200.0 + 60.0 * sn);
    }
    let g = path(fac, &[(&star, true)], true);
    let gold = solid(rt, color(1.0, 0.84, 0.0, 1.0));
    fill_geometry(rt, g, gold);
    let fg: extern "system" fn(Obj, Obj, Obj, f32, Obj) = unsafe { m(rt, RT_DRAW_GEOMETRY) };
    fg(rt, g, black, 2.0, null_mut());
    end_draw(rt);
    for o in [g, gold, black, green, red, sky, coll, fac] {
        release(o);
    }
}

/// sin and cos by Taylor series (|a| up to 2 pi, enough for the demo).
fn sincos(a: f32) -> (f32, f32) {
    let pi = core::f32::consts::PI;
    let mut x = a % (2.0 * pi);
    if x > pi {
        x -= 2.0 * pi;
    }
    if x < -pi {
        x += 2.0 * pi;
    }
    let (mut s, mut c) = (0.0f32, 0.0f32);
    let (mut ts, mut tc) = (x, 1.0f32);
    for k in 0..10 {
        s += ts;
        c += tc;
        let n = 2.0 * k as f32;
        ts *= -x * x / ((n + 2.0) * (n + 3.0));
        tc *= -x * x / ((n + 1.0) * (n + 2.0));
    }
    (s, c)
}

/// `d2d1_test window`: the demo window until the key q.
fn demo(f: Obj) -> Result<(), u32> {
    let hwnd = window(WS_OVERLAPPEDWINDOW | WS_VISIBLE, 100, 100, 400, 300, "Direct2D");
    if hwnd == 0 {
        return Err(1);
    }
    let p = props(0, 0);
    let hp = HwndProps { hwnd, w: 0, h: 0, present: 0 };
    let (r, rt) = obj2(f, F_CREATE_HWND_TARGET, &p as *const TargetProps, &hp as *const HwndProps);
    if r != S_OK {
        return Err(2);
    }
    DEMO.store(rt as usize, Ordering::Relaxed);
    // SAFETY: the window's message loop, with our message buffer.
    unsafe {
        let mut msg: Msg = core::mem::zeroed();
        while GetMessageW(&mut msg, 0, 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    DEMO.store(0, Ordering::Relaxed);
    release(rt);
    Ok(())
}

fn has_arg(arg: &str) -> bool {
    // SAFETY: the process's command line, NUL-terminated.
    let line = unsafe { GetCommandLineW() };
    let mut n = 0;
    // SAFETY: as above.
    while unsafe { *line.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: as above.
    let units = unsafe { core::slice::from_raw_parts(line, n) };
    let a: [u16; 16] = wide(arg);
    units.windows(arg.len()).any(|w| w == &a[..arg.len()])
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let mut wic = null_mut();
    // SAFETY: valid arguments (WINCODEC_SDK_VERSION2).
    unsafe { c.hr(WICCreateImagingFactory_Proxy(0x0237, &mut wic), S_OK)? };
    exports(&mut c)?;
    let f = factory(&mut c)?;
    if has_arg(" window") {
        return demo(f);
    }
    matrices(&mut c)?;
    wic_target(&mut c, f, wic)?;
    drawing(&mut c, f, wic)?;
    paints(&mut c, f, wic)?;
    brushes(&mut c, f, wic)?;
    geometries(&mut c, f, wic)?;
    dc_target(&mut c, f)?;
    hwnd_target(&mut c, f)?;
    release(f);
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
