//! gala's gdi32.dll: device contexts that draw into pixel memory (a
//! window's client area, which user32 hands over with
//! `GalaCreateSurfaceDC` and presents to the desktop; a bitmap selected
//! into a memory DC; nothing for the display DC, which answers questions:
//! device caps, text metrics), the objects (brushes, pens, fonts kept as
//! LOGFONTW, bitmaps: 32-bit device-dependent ones and 24/32-bit DIB
//! sections), drawing (PatBlt, Rectangle, pixels, BitBlt/StretchBlt with
//! any ternary raster operation) and text in Noto Sans Mono scaled to the
//! font's height (`font`). OpenGL's pixel formats and SwapBuffers go to
//! the process's opengl32 (`opengl`). Coordinates go through the window and viewport
//! origins and are clipped to the DC's clip rectangle. Pixels are
//! 0x00RRGGBB; COLORREFs 0x00BBGGRR. Objects live in a fixed table; a
//! handle is 0x0100_0000 plus the slot (stock objects keep theirs).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;

mod absent;
mod blend;
mod dib;
mod draw;
mod font;
mod older;
mod opengl;
mod palette;
mod region;
mod shapes;
mod state;
mod surface;
mod unimplemented;

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, Ordering};
use font::LogFont;
use surface::Surface;

// The global allocator (for fontdue, which rasterizes Selawik).
use gala_heap as _;

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import. On x86, ntrt's.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

// The 32-bit build (SysWOW64's gdi32) takes the memory routines, 64-bit
// division helpers and unwind personality from ntrt's x86 runtime: a
// raw-dylib import does not give the symbols the compiler calls there
// (ARCHITECTURE "Plan: WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

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

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaScreenInfo(info: *mut [i32; 8]) -> u32;
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn VirtualAlloc(at: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    fn VirtualFree(at: *mut c_void, size: usize, kind: u32) -> i32;
    pub(crate) fn MapViewOfFile(
        mapping: *mut c_void,
        access: u32,
        offset_high: u32,
        offset_low: u32,
        size: usize,
    ) -> *mut c_void;
    fn UnmapViewOfFile(at: *const c_void) -> i32;
}

type Handle = *mut c_void;

const SLOTS: usize = 1024;
const BASE: usize = 0x0100_0000;
const STOCK: usize = 22;
/// The 1 x 1 monochrome bitmap memory DCs start with (not a documented
/// GetStockObject index; kept past them).
const DEFAULT_BITMAP: usize = 21;

#[derive(Clone, Copy)]
pub(crate) struct Bitmap {
    pub surface: Surface,
    /// What GetObject reports: 1 or 32 for device-dependent bitmaps, the
    /// DIB's own depth for DIB sections.
    pub bpp: u16,
    pub dib: bool,
    /// Memory to free with the bitmap (VirtualAlloc'd).
    pub owned: bool,
    /// The view of a file mapping the bits are in (a DIB section over a
    /// caller's section), unmapped with the bitmap; null for none.
    pub view: *mut u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DcKind {
    Window,
    Memory,
    Display,
}

#[derive(Clone, Copy)]
pub(crate) struct Dc {
    pub kind: DcKind,
    pub surface: Surface,
    /// A memory DC's bitmap (slot).
    pub bitmap: usize,
    pub text: u32,
    pub background: u32,
    /// TRANSPARENT (1) or OPAQUE (2).
    pub mode: i32,
    pub brush: usize,
    pub pen: usize,
    pub font: usize,
    /// Device = logical - window origin + viewport origin.
    pub window_org: (i32, i32),
    pub viewport_org: (i32, i32),
    /// The clip rectangle in device coordinates (none: the surface).
    pub clip: Option<region::Clip>,
    /// MoveToEx's position (logical).
    pub pos: (i32, i32),
    pub align: u32,
    /// SetROP2's mix (R2_COPYPEN 13), SetPolyFillMode's mode (ALTERNATE
    /// 1), the brush origin, the graphics mode (GM_COMPATIBLE 1), the
    /// layout, and the world transform (eM11, eM12, eM21, eM22, eDx, eDy;
    /// kept, not applied).
    pub rop2: i32,
    pub poly_fill: i32,
    pub brush_org: (i32, i32),
    pub graphics_mode: i32,
    pub layout: u32,
    pub xform: [f32; 6],
    /// SetStretchBltMode's mode (kept; StretchBlt samples the nearest
    /// pixel in every mode).
    pub stretch: i32,
    /// The mapping mode (MM_TEXT 1 .. MM_ANISOTROPIC 8) and the window and
    /// viewport extents: device = (logical - window origin) * viewport
    /// extent / window extent + viewport origin.
    pub map_mode: i32,
    pub window_ext: (i32, i32),
    pub viewport_ext: (i32, i32),
    /// A child window's DC: its offset in the top-level window's surface
    /// (device = logical - window origin + viewport origin + base) and the
    /// part of the surface it may draw on (its visible rectangle).
    pub base: (i32, i32),
    pub bounds: [i32; 4],
    /// The selected palette (slot).
    pub palette: usize,
    /// SetDCBrushColor's and SetDCPenColor's colours (COLORREF).
    pub dc_brush: u32,
    pub dc_pen: u32,
}

/// v * num / den rounded to the nearest (halves away from zero).
fn scale(v: i32, num: i32, den: i32) -> i32 {
    if num == den {
        return v;
    }
    let (p, d) = (i64::from(v) * i64::from(num), i64::from(den));
    if d == 0 {
        return v;
    }
    let q = (2 * p.abs() + d.abs()) / (2 * d.abs());
    (if (p < 0) != (d < 0) { -q } else { q }) as i32
}

impl Dc {
    /// Logical (x, y) in the surface's pixels (the DC's device
    /// coordinates plus a child window's offset).
    pub fn device_point(&self, x: i32, y: i32) -> (i32, i32) {
        let (x, y) = self.logical_to_device(x, y);
        (x + self.base.0, y + self.base.1)
    }

    /// LPtoDP: logical to the DC's device coordinates.
    pub fn logical_to_device(&self, x: i32, y: i32) -> (i32, i32) {
        (
            scale(x - self.window_org.0, self.viewport_ext.0, self.window_ext.0)
                + self.viewport_org.0,
            scale(y - self.window_org.1, self.viewport_ext.1, self.window_ext.1)
                + self.viewport_org.1,
        )
    }

    /// DPtoLP: the DC's device coordinates to logical.
    pub fn device_to_logical(&self, x: i32, y: i32) -> (i32, i32) {
        (
            scale(x - self.viewport_org.0, self.window_ext.0, self.viewport_ext.0)
                + self.window_org.0,
            scale(y - self.viewport_org.1, self.window_ext.1, self.viewport_ext.1)
                + self.window_org.1,
        )
    }

    /// Whether device pixel (x, y) may be drawn.
    pub fn visible(&self, x: i32, y: i32) -> bool {
        let b = self.bounds;
        self.surface.contains(x, y)
            && x >= b[0]
            && y >= b[1]
            && x < b[2]
            && y < b[3]
            && self.clip.is_none_or(|c| c.contains(x, y))
    }

    /// Fills a device rectangle (clipped).
    pub fn fill(&self, x: i32, y: i32, w: i32, h: i32, c: u32) {
        let mut r = [
            x.max(0),
            y.max(0),
            x.saturating_add(w).min(self.surface.width),
            y.saturating_add(h).min(self.surface.height),
        ];
        let b = self.bounds;
        r = [r[0].max(b[0]), r[1].max(b[1]), r[2].min(b[2]), r[3].min(b[3])];
        let whole = [r];
        let parts: &[[i32; 4]] = match &self.clip {
            Some(clip) => &clip.rects[..clip.count],
            None => &whole,
        };
        for p in parts {
            let q = [r[0].max(p[0]), r[1].max(p[1]), r[2].min(p[2]), r[3].min(p[3])];
            for yy in q[1]..q[3] {
                for xx in q[0]..q[2] {
                    self.surface.put(xx, yy, c);
                }
            }
        }
    }
}

/// `repr(u8)` with Free first: an unused slot is all zeros, so the table
/// goes in .bss instead of the DLL's data.
#[derive(Clone, Copy)]
#[repr(u8)]
// A DC (with its clip rectangles) is the largest; the table is a fixed
// array of these, so boxing would only add allocations.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Object {
    Free = 0,
    Brush {
        color: u32,
        null: bool,
    },
    Pen {
        color: u32,
        null: bool,
        width: i32,
        style: i32,
    },
    Font(LogFont),
    Bitmap(Bitmap),
    Dc(Dc),
    /// An index into the palette store.
    Palette(usize),
    /// An index into the region store.
    Region(usize),
}

struct Table {
    lock: AtomicBool,
    slots: UnsafeCell<[Object; SLOTS]>,
    stock: UnsafeCell<[usize; STOCK]>,
    ready: UnsafeCell<bool>,
}

// SAFETY: every access is under `lock`.
unsafe impl Sync for Table {}

static TABLE: Table = Table {
    lock: AtomicBool::new(false),
    slots: UnsafeCell::new([Object::Free; SLOTS]),
    stock: UnsafeCell::new([0; STOCK]),
    ready: UnsafeCell::new(false),
};

/// How many stock objects the table starts with (the default palette
/// goes after them).
const fn objects_len() -> usize {
    17
}

/// The default bitmap's one pixel.
static mut DEFAULT_PIXEL: u32 = 0;

/// Runs `f` on the object table (with the stock objects made).
pub(crate) fn with<R>(f: impl FnOnce(&mut [Object; SLOTS], &[usize; STOCK]) -> R) -> R {
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = unsafe {
        let slots = &mut *TABLE.slots.get();
        let stock = &mut *TABLE.stock.get();
        if !*TABLE.ready.get() {
            let gui = Object::Font(LogFont::default_gui());
            let one = Surface {
                bits: (&raw mut DEFAULT_PIXEL).cast(),
                width: 1,
                height: 1,
                stride: 4,
                bpp: 32,
                palette: [0, 0x00ff_ffff],
                bottom_up: false,
                mono: true,
                bounds: core::ptr::null(),
            };
            // WHITE, LTGRAY, GRAY, DKGRAY, BLACK, NULL brushes; WHITE,
            // BLACK, NULL pens; the fonts (one face for all); the default
            // bitmap.
            let objects = [
                (0, Object::Brush { color: 0xffffff, null: false }),
                (1, Object::Brush { color: 0xc0c0c0, null: false }),
                (2, Object::Brush { color: 0x808080, null: false }),
                (3, Object::Brush { color: 0x404040, null: false }),
                (4, Object::Brush { color: 0x000000, null: false }),
                (5, Object::Brush { color: 0, null: true }),
                (6, Object::Pen { color: 0xffffff, null: false, width: 1, style: 0 }),
                (7, Object::Pen { color: 0x000000, null: false, width: 1, style: 0 }),
                (8, Object::Pen { color: 0, null: true, width: 1, style: 5 }),
                (10, gui),
                (11, gui),
                (12, gui),
                (13, gui),
                (14, gui),
                (16, gui),
                (17, gui),
                (
                    DEFAULT_BITMAP,
                    Object::Bitmap(Bitmap {
                        surface: one,
                        bpp: 1,
                        dib: false,
                        owned: false,
                        view: core::ptr::null_mut(),
                    }),
                ),
            ];
            for (slot, (index, object)) in objects.into_iter().enumerate() {
                slots[slot] = object;
                stock[index] = slot;
            }
            // DEFAULT_PALETTE: the 20 static colours' first and last
            // (black, white) are what matters on a true-colour display.
            let next = objects_len();
            slots[next] = Object::Palette(usize::MAX);
            stock[15] = next;
            *TABLE.ready.get() = true;
        }
        f(slots, stock)
    };
    TABLE.lock.store(false, Ordering::Release);
    r
}

pub(crate) fn handle(slot: usize) -> Handle {
    (BASE + slot) as Handle
}

pub(crate) fn slot_of(h: Handle) -> Option<usize> {
    let v = (h as usize).checked_sub(BASE)?;
    (v < SLOTS).then_some(v)
}

pub(crate) fn new_object(object: Object) -> Handle {
    with(|slots, _| match slots.iter().position(|o| matches!(o, Object::Free)) {
        Some(i) => {
            slots[i] = object;
            handle(i)
        }
        None => core::ptr::null_mut(),
    })
}

/// COLORREF (0x00BBGGRR) to a pixel (0x00RRGGBB); the swap is its own
/// inverse.
pub(crate) fn pixel(color: u32) -> u32 {
    (color & 0xff) << 16 | (color & 0xff00) | (color >> 16) & 0xff
}

/// Runs `f` on DC `h`'s state, if it is one.
pub(crate) fn with_dc<R>(
    h: Handle,
    f: impl FnOnce(&mut Dc, &mut [Object; SLOTS]) -> R,
) -> Option<R> {
    let i = slot_of(h)?;
    with(|slots, _| {
        let Object::Dc(mut dc) = slots[i] else { return None };
        let r = f(&mut dc, slots);
        if let Object::Dc(_) = slots[i] {
            slots[i] = Object::Dc(dc);
        }
        Some(r)
    })
}

pub(crate) fn unsupported(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; "unsupported" (2).
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
}

/// The screen's size (1280 x 800 without one, as user32 assumes).
pub(crate) fn screen() -> (i32, i32) {
    let mut info = [0i32; 8];
    // SAFETY: our buffer.
    if unsafe { GalaScreenInfo(&mut info) } == 0 { (info[2], info[3]) } else { (1280, 800) }
}

pub(crate) fn alloc(bytes: usize) -> *mut u8 {
    // SAFETY: a plain call; MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE
    // (zeroed).
    unsafe { VirtualAlloc(core::ptr::null_mut(), bytes.max(1), 0x3000, 4).cast() }
}

fn new_dc(kind: DcKind, surface: Surface, bitmap: usize) -> Handle {
    let (brush, pen, font) = with(|_, stock| (stock[0], stock[7], stock[13]));
    let hdc = new_object(Object::Dc(Dc {
        kind,
        surface,
        bitmap,
        text: 0,
        background: 0xffffff,
        mode: 2,
        brush,
        pen,
        font,
        window_org: (0, 0),
        viewport_org: (0, 0),
        clip: None,
        pos: (0, 0),
        align: 0,
        base: (0, 0),
        bounds: [i32::MIN, i32::MIN, i32::MAX, i32::MAX],
        palette: with(|_, stock| stock[15]),
        stretch: 1, // BLACKONWHITE
        rop2: 13,
        poly_fill: 1,
        brush_org: (0, 0),
        graphics_mode: 1,
        layout: 0,
        xform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        map_mode: 1,
        window_ext: (1, 1),
        viewport_ext: (1, 1),
        dc_brush: 0xffffff,
        dc_pen: 0,
    }));
    // A new DC accumulates nothing yet (the slot may have been another
    // DC's).
    if let Some(i) = slot_of(hdc) {
        BOUNDS[i].reset();
    }
    hdc
}

// ---- gala's own exports for user32 ----

/// A DC drawing into `width` x `height` pixels at `pixels` (rows `stride`
/// pixels apart): a window's client area.
#[unsafe(no_mangle)]
pub extern "system" fn GalaCreateSurfaceDC(
    pixels: *mut u32,
    width: i32,
    height: i32,
    stride: i32,
) -> Handle {
    let surface = Surface {
        bits: pixels.cast(),
        width,
        height,
        stride: stride * 4,
        bpp: 32,
        palette: [0, 0x00ff_ffff],
        bottom_up: false,
        mono: false,
        bounds: core::ptr::null(),
    };
    new_dc(DcKind::Window, surface, 0)
}

/// A child window's DC: drawing into a top-level window's client area
/// (`width` x `height` at `pixels`, rows `stride` pixels apart) at offset
/// (`x`, `y`), limited to the rectangle `bounds` of that area.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn GalaCreateChildDC(
    pixels: *mut u32,
    width: i32,
    height: i32,
    stride: i32,
    x: i32,
    y: i32,
    bounds: *const [i32; 4],
) -> Handle {
    let hdc = GalaCreateSurfaceDC(pixels, width, height, stride);
    // SAFETY: the caller's RECT.
    let b = if bounds.is_null() { [0, 0, width, height] } else { unsafe { *bounds } };
    with_dc(hdc, |dc, _| {
        dc.base = (x, y);
        dc.bounds = b;
    });
    hdc
}

#[unsafe(no_mangle)]
pub extern "system" fn GalaDeleteSurfaceDC(hdc: Handle) -> i32 {
    DeleteDC(hdc)
}

// ---- Device contexts ----

/// Whether a UTF-16 string equals ASCII `s` (case-insensitively).
fn wide_is(w: *const u16, s: &[u8]) -> bool {
    if w.is_null() {
        return false;
    }
    for (i, &c) in s.iter().enumerate() {
        // SAFETY: the caller's NUL-terminated string, read while it matches.
        let u = unsafe { *w.add(i) };
        if u > 0x7f || !(u as u8).eq_ignore_ascii_case(&c) {
            return false;
        }
    }
    // SAFETY: as above.
    unsafe { *w.add(s.len()) == 0 }
}

/// CreateDCW(driver, device, output, devmode): the display ("DISPLAY",
/// or \\.\DISPLAY1); printers do not exist here (NULL, ledger).
#[unsafe(no_mangle)]
pub extern "system" fn CreateDCW(
    driver: *const u16,
    device: *const u16,
    _output: *const u16,
    _mode: *const c_void,
) -> Handle {
    if wide_is(driver, b"DISPLAY")
        || wide_is(device, b"\\\\.\\DISPLAY1")
        || (driver.is_null() && device.is_null())
    {
        return new_dc(DcKind::Display, Surface::NONE, 0);
    }
    unsupported(b"gdi32.dll!CreateDCW(printer)", 0);
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateICW(
    driver: *const u16,
    device: *const u16,
    output: *const u16,
    mode: *const c_void,
) -> Handle {
    CreateDCW(driver, device, output, mode)
}

/// CreateCompatibleDC: a memory DC with the 1 x 1 monochrome bitmap.
#[unsafe(no_mangle)]
pub extern "system" fn CreateCompatibleDC(_hdc: Handle) -> Handle {
    let (slot, surface) = with(|slots, stock| {
        let s = stock[DEFAULT_BITMAP];
        let Object::Bitmap(b) = slots[s] else { return (s, Surface::NONE) };
        (s, b.surface)
    });
    new_dc(DcKind::Memory, surface, slot)
}

#[unsafe(no_mangle)]
pub extern "system" fn DeleteDC(hdc: Handle) -> i32 {
    let Some(i) = slot_of(hdc) else { return 0 };
    with(|slots, _| {
        if !matches!(slots[i], Object::Dc(_)) {
            return 0;
        }
        slots[i] = Object::Free;
        state::forget(i);
        1
    })
}

/// GetDeviceCaps: the display's (memory DCs are compatible with it).
#[unsafe(no_mangle)]
pub extern "system" fn GetDeviceCaps(hdc: Handle, index: i32) -> i32 {
    if with_dc(hdc, |_, _| ()).is_none() {
        return 0;
    }
    let (w, h) = screen();
    match index {
        2 => 1,                                                    // TECHNOLOGY: DT_RASDISPLAY
        4 => (w * 254 + 480) / 960,                                // HORZSIZE (mm)
        6 => (h * 254 + 480) / 960,                                // VERTSIZE
        8 | 118 => w,                                              // HORZRES, DESKTOPHORZRES
        10 | 117 => h,                                             // VERTRES, DESKTOPVERTRES
        12 => 32,                                                  // BITSPIXEL
        14 => 1,                                                   // PLANES
        24 => -1,                                                  // NUMCOLORS: more than 256
        38 => 0x0001 | 0x0008 | 0x0080 | 0x0200 | 0x0800 | 0x2000, // RASTERCAPS
        88 | 90 => 96,                                             // LOGPIXELSX/Y
        104 => 0,                                                  // SIZEPALETTE
        108 => 24,                                                 // COLORRES
        116 => 60,                                                 // VREFRESH
        120 => 0x3, // SHADEBLENDCAPS: const and pixel alpha
        _ => 0,
    }
}

// ---- Objects ----

#[unsafe(no_mangle)]
pub extern "system" fn GetStockObject(index: i32) -> Handle {
    if !(0..=20).contains(&index) {
        return core::ptr::null_mut();
    }
    let slot = with(|slots, stock| {
        let s = stock[index as usize];
        (index == 0 || s != 0).then_some(s).filter(|&s| !matches!(slots[s], Object::Free))
    });
    slot.map_or(core::ptr::null_mut(), handle)
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateSolidBrush(color: u32) -> Handle {
    new_object(Object::Brush { color: pixel(color), null: false })
}

/// LOGBRUSH: style, colour, hatch.
#[repr(C)]
pub struct LogBrush {
    style: u32,
    color: u32,
    hatch: usize,
}

/// CreateBrushIndirect: BS_SOLID (0) as CreateSolidBrush, BS_NULL (1) a
/// brush that paints nothing; BS_HATCHED (2) is painted solid (recorded);
/// pattern and DIB brushes are not built (null, recorded).
#[unsafe(no_mangle)]
pub extern "system" fn CreateBrushIndirect(lb: *const LogBrush) -> Handle {
    if lb.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: the caller's LOGBRUSH.
    let (style, color) = unsafe { ((*lb).style, (*lb).color) };
    match style {
        0 => CreateSolidBrush(color),
        1 => new_object(Object::Brush { color: 0, null: true }),
        2 => {
            unsupported(b"gdi32.dll!CreateBrushIndirect(BS_HATCHED: painted solid)", 0);
            CreateSolidBrush(color)
        }
        s => {
            unsupported(b"gdi32.dll!CreateBrushIndirect(style)", u64::from(s));
            core::ptr::null_mut()
        }
    }
}

/// CreatePen(style, width, color): PS_NULL (5) draws nothing; other
/// styles draw solid.
#[unsafe(no_mangle)]
pub extern "system" fn CreatePen(style: i32, width: i32, color: u32) -> Handle {
    new_object(Object::Pen {
        color: pixel(color),
        null: style & 0xf == 5,
        width: width.max(1),
        style,
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateFontIndirectW(lf: *const LogFont) -> Handle {
    if lf.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: the caller's LOGFONTW.
    new_object(Object::Font(unsafe { *lf }))
}

/// CreateFontW: a LOGFONTW from its fields.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn CreateFontW(
    height: i32,
    width: i32,
    escapement: i32,
    orientation: i32,
    weight: i32,
    italic: u32,
    underline: u32,
    strike_out: u32,
    char_set: u32,
    out_precision: u32,
    clip_precision: u32,
    quality: u32,
    pitch_and_family: u32,
    face: *const u16,
) -> Handle {
    let mut lf = LogFont {
        height,
        width,
        escapement,
        orientation,
        weight,
        italic: italic as u8,
        underline: underline as u8,
        strike_out: strike_out as u8,
        char_set: char_set as u8,
        out_precision: out_precision as u8,
        clip_precision: clip_precision as u8,
        quality: quality as u8,
        pitch_and_family: pitch_and_family as u8,
        face: [0; 32],
    };
    if !face.is_null() {
        for i in 0..31 {
            // SAFETY: the caller's NUL-terminated name, up to 31 characters.
            let c = unsafe { *face.add(i) };
            if c == 0 {
                break;
            }
            lf.face[i] = c;
        }
    }
    new_object(Object::Font(lf))
}

/// DeleteObject: frees a brush, pen, font, bitmap (not while a DC has it
/// selected) or DC; stock objects stay.
#[unsafe(no_mangle)]
pub extern "system" fn DeleteObject(h: Handle) -> i32 {
    let Some(i) = slot_of(h) else { return 0 };
    let freed = with(|slots, stock| {
        if matches!(slots[i], Object::Free) {
            return None;
        }
        if stock.contains(&i) {
            return Some(None);
        }
        if let Object::Palette(p) = slots[i]
            && p != usize::MAX
        {
            if slots.iter().any(|o| matches!(o, Object::Dc(d) if d.palette == i)) {
                return None;
            }
            slots[i] = Object::Free;
            palette::free(p);
            return Some(None);
        }
        if let Object::Region(id) = slots[i] {
            slots[i] = Object::Free;
            region::free(id);
            return Some(None);
        }
        if let Object::Bitmap(b) = slots[i] {
            if slots
                .iter()
                .any(|o| matches!(o, Object::Dc(d) if d.kind == DcKind::Memory && d.bitmap == i))
            {
                return None;
            }
            slots[i] = Object::Free;
            return Some(if !b.view.is_null() {
                Some((b.view, true))
            } else {
                b.owned.then_some((b.surface.bits, false))
            });
        }
        slots[i] = Object::Free;
        Some(None)
    });
    match freed {
        None => 0,
        Some(memory) => {
            match memory {
                // SAFETY: the bitmap's own view, no longer referenced.
                Some((view, true)) => unsafe {
                    UnmapViewOfFile(view.cast());
                },
                // SAFETY: the bitmap's own memory, no longer referenced.
                Some((bits, false)) => unsafe {
                    VirtualFree(bits.cast(), 0, 0x8000);
                },
                None => {}
            }
            1
        }
    }
}

/// SelectObject(hdc, object): the previously selected object of its kind
/// (a bitmap only into a memory DC).
#[unsafe(no_mangle)]
pub extern "system" fn SelectObject(hdc: Handle, object: Handle) -> Handle {
    let Some(new) = slot_of(object) else { return core::ptr::null_mut() };
    with_dc(hdc, |dc, slots| {
        let field = match slots[new] {
            Object::Brush { .. } => &mut dc.brush,
            Object::Pen { .. } => &mut dc.pen,
            Object::Font(_) => &mut dc.font,
            Object::Bitmap(b) if dc.kind == DcKind::Memory => {
                // Not if another DC has it.
                let elsewhere = slots.iter().any(|o| matches!(o, Object::Dc(d) if d.kind == DcKind::Memory && d.bitmap == new && !core::ptr::eq(d, dc)));
                if elsewhere && new != dc.bitmap {
                    return core::ptr::null_mut();
                }
                // The DC's bounds accumulation stays on.
                dc.surface = surface::Surface { bounds: dc.surface.bounds, ..b.surface };
                dc.clip = None;
                &mut dc.bitmap
            }
            _ => return core::ptr::null_mut(),
        };
        handle(core::mem::replace(field, new))
    })
    .unwrap_or(core::ptr::null_mut())
}

/// GetObjectType: OBJ_PEN 1, OBJ_BRUSH 2, OBJ_DC 3, OBJ_FONT 6,
/// OBJ_BITMAP 7, OBJ_MEMDC 10; 0 for others.
#[unsafe(no_mangle)]
pub extern "system" fn GetObjectType(h: Handle) -> u32 {
    let Some(i) = slot_of(h) else { return 0 };
    with(|slots, _| match slots[i] {
        Object::Free => 0,
        Object::Pen { style, .. } if style & shapes::EXT_PEN != 0 => 11,
        Object::Pen { .. } => 1,
        Object::Brush { .. } => 2,
        Object::Dc(d) if d.kind == DcKind::Memory => 10,
        Object::Dc(_) => 3,
        Object::Font(_) => 6,
        Object::Bitmap(_) => 7,
        Object::Palette(_) => 5,
        Object::Region(_) => 8,
    })
}

/// GetCurrentObject(hdc, OBJ_PEN/BRUSH/FONT/BITMAP).
#[unsafe(no_mangle)]
pub extern "system" fn GetCurrentObject(hdc: Handle, kind: u32) -> Handle {
    with_dc(hdc, |dc, _| match kind {
        1 => handle(dc.pen),
        2 => handle(dc.brush),
        6 => handle(dc.font),
        7 if dc.kind == DcKind::Memory => handle(dc.bitmap),
        _ => core::ptr::null_mut(),
    })
    .unwrap_or(core::ptr::null_mut())
}

/// Writes `bytes` to `out` (at most `size`); the count, or the size
/// needed when `out` is NULL.
fn give(out: *mut u8, size: i32, bytes: &[u8]) -> i32 {
    if out.is_null() {
        return bytes.len() as i32;
    }
    let n = (size.max(0) as usize).min(bytes.len());
    // SAFETY: the caller's buffer of `size` bytes.
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), out, n) };
    n as i32
}

/// GetObjectW: LOGBRUSH, LOGPEN, LOGFONTW, BITMAP or (asked with room for
/// it) DIBSECTION.
#[unsafe(no_mangle)]
pub extern "system" fn GetObjectW(h: Handle, size: i32, out: *mut u8) -> i32 {
    // The sizes with a pointer in them: LOGBRUSH (lbHatch), BITMAP (bmBits
    // at 24 on x64, 20 on x86) and DIBSECTION (a BITMAP, the
    // BITMAPINFOHEADER after it, dsBitfields, dshSection, dsOffset: 104
    // bytes on x64, 84 on x86).
    const P: usize = size_of::<usize>();
    const LOGBRUSH: usize = 8 + P;
    const BM_BITS: usize = if P == 8 { 24 } else { 20 };
    const BITMAP: usize = BM_BITS + P;
    const DIBSECTION: usize = if P == 8 { 104 } else { 84 };
    let Some(i) = slot_of(h) else { return 0 };
    let object = with(|slots, _| slots[i]);
    let mut buf = [0u8; 104];
    let put = |b: &mut [u8; 104], at: usize, v: &[u8]| b[at..at + v.len()].copy_from_slice(v);
    match object {
        Object::Brush { color, null } => {
            put(&mut buf, 0, &u32::from(null).to_le_bytes()); // BS_SOLID 0, BS_NULL 1
            put(&mut buf, 4, &pixel(color).to_le_bytes());
            give(out, size, &buf[..LOGBRUSH])
        }
        Object::Pen { color, null, width, style } => {
            put(&mut buf, 0, &(if null { 5 } else { style as u32 }).to_le_bytes());
            put(&mut buf, 4, &width.to_le_bytes());
            put(&mut buf, 12, &pixel(color).to_le_bytes());
            give(out, size, &buf[..16])
        }
        Object::Font(lf) => {
            // SAFETY: LOGFONTW is plain data of 92 bytes.
            let bytes =
                unsafe { core::slice::from_raw_parts((&lf as *const LogFont).cast::<u8>(), 92) };
            give(out, size, bytes)
        }
        Object::Bitmap(b) => {
            let s = b.surface;
            let width_bytes =
                if b.dib { s.stride } else { ((s.width * i32::from(b.bpp) + 15) / 16) * 2 };
            put(&mut buf, 4, &s.width.to_le_bytes());
            put(&mut buf, 8, &s.height.to_le_bytes());
            put(&mut buf, 12, &width_bytes.to_le_bytes());
            put(&mut buf, 16, &1u16.to_le_bytes());
            put(&mut buf, 18, &b.bpp.to_le_bytes());
            let bits = if b.dib { s.bits as usize } else { 0 };
            put(&mut buf, BM_BITS, &bits.to_le_bytes());
            if b.dib && (out.is_null() || size as usize >= DIBSECTION) {
                // The BITMAPINFOHEADER (bottom-up: positive height).
                let at = BITMAP;
                put(&mut buf, at, &40u32.to_le_bytes());
                put(&mut buf, at + 4, &s.width.to_le_bytes());
                let h = if s.bottom_up { s.height } else { -s.height };
                put(&mut buf, at + 8, &h.to_le_bytes());
                put(&mut buf, at + 12, &1u16.to_le_bytes());
                put(&mut buf, at + 14, &b.bpp.to_le_bytes());
                put(&mut buf, at + 20, &((s.stride * s.height) as u32).to_le_bytes());
                return give(out, size, &buf[..DIBSECTION]);
            }
            give(out, size, &buf[..BITMAP])
        }
        Object::Palette(p) => {
            let count = if p == usize::MAX { 20u16 } else { palette::count(p) };
            give(out, size, &count.to_le_bytes())
        }
        Object::Dc(_) | Object::Free | Object::Region(_) => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn SetTextColor(hdc: Handle, color: u32) -> u32 {
    with_dc(hdc, |dc, _| pixel(core::mem::replace(&mut dc.text, pixel(color))))
        .unwrap_or(0xffff_ffff)
}

#[unsafe(no_mangle)]
pub extern "system" fn SetBkColor(hdc: Handle, color: u32) -> u32 {
    with_dc(hdc, |dc, _| pixel(core::mem::replace(&mut dc.background, pixel(color))))
        .unwrap_or(0xffff_ffff)
}

/// SetStretchBltMode: the previous mode. Any value is kept, as on Windows
/// (25H2 keeps 0, 5 and -1 and returns them, the last error untouched);
/// StretchBlt samples the nearest pixel whatever it is.
#[unsafe(no_mangle)]
pub extern "system" fn SetStretchBltMode(hdc: Handle, mode: i32) -> i32 {
    with_dc(hdc, |dc, _| core::mem::replace(&mut dc.stretch, mode)).unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetStretchBltMode(hdc: Handle) -> i32 {
    with_dc(hdc, |dc, _| dc.stretch).unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn SetBkMode(hdc: Handle, mode: i32) -> i32 {
    if mode != 1 && mode != 2 {
        return 0;
    }
    with_dc(hdc, |dc, _| core::mem::replace(&mut dc.mode, mode)).unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetTextColor(hdc: Handle) -> u32 {
    with_dc(hdc, |dc, _| pixel(dc.text)).unwrap_or(0xffff_ffff)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetBkColor(hdc: Handle) -> u32 {
    with_dc(hdc, |dc, _| pixel(dc.background)).unwrap_or(0xffff_ffff)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetBkMode(hdc: Handle) -> i32 {
    with_dc(hdc, |dc, _| dc.mode).unwrap_or(0)
}

// ---- Coordinates ----

/// Sets an origin, writing the previous one to `prev` (a POINT) if given.
fn set_origin(
    hdc: Handle,
    x: i32,
    y: i32,
    prev: *mut [i32; 2],
    viewport: bool,
    relative: bool,
) -> i32 {
    with_dc(hdc, |dc, _| {
        let o = if viewport { &mut dc.viewport_org } else { &mut dc.window_org };
        if !prev.is_null() {
            // SAFETY: the caller's POINT.
            unsafe { *prev = [o.0, o.1] };
        }
        *o = if relative { (o.0 + x, o.1 + y) } else { (x, y) };
        1
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn SetViewportOrgEx(hdc: Handle, x: i32, y: i32, prev: *mut [i32; 2]) -> i32 {
    set_origin(hdc, x, y, prev, true, false)
}

#[unsafe(no_mangle)]
pub extern "system" fn OffsetViewportOrgEx(
    hdc: Handle,
    x: i32,
    y: i32,
    prev: *mut [i32; 2],
) -> i32 {
    set_origin(hdc, x, y, prev, true, true)
}

#[unsafe(no_mangle)]
pub extern "system" fn SetWindowOrgEx(hdc: Handle, x: i32, y: i32, prev: *mut [i32; 2]) -> i32 {
    set_origin(hdc, x, y, prev, false, false)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetViewportOrgEx(hdc: Handle, out: *mut [i32; 2]) -> i32 {
    with_dc(hdc, |dc, _| {
        // SAFETY: the caller's POINT.
        unsafe { *out = [dc.viewport_org.0, dc.viewport_org.1] };
        1
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetWindowOrgEx(hdc: Handle, out: *mut [i32; 2]) -> i32 {
    with_dc(hdc, |dc, _| {
        // SAFETY: the caller's POINT.
        unsafe { *out = [dc.window_org.0, dc.window_org.1] };
        1
    })
    .unwrap_or(0)
}

/// The extents of a fixed mapping mode: window (in the mode's units) and
/// viewport (device pixels, y up except for MM_TEXT).
fn fixed_extents(mode: i32) -> ((i32, i32), (i32, i32)) {
    let (w, h) = screen();
    // HORZSIZE/VERTSIZE in mm (as GetDeviceCaps) times the mode's units
    // per mm (in thousandths).
    let mm = |px: i32| (px * 254 + 480) / 960;
    let per = match mode {
        2 => 10_000,  // MM_LOMETRIC: 0.1 mm
        3 => 100_000, // MM_HIMETRIC: 0.01 mm
        4 => 3_937,   // MM_LOENGLISH: 0.01 inch
        5 => 39_370,  // MM_HIENGLISH: 0.001 inch
        6 => 56_693,  // MM_TWIPS: 1/1440 inch
        _ => return ((1, 1), (1, 1)),
    };
    let units = |px: i32| (i64::from(mm(px)) * per / 1000) as i32;
    ((units(w), units(h)), (w, -h))
}

/// MM_ISOTROPIC: the viewport extent shrinks on the axis that would
/// scale more, so both units have the same size.
fn isotropic(dc: &mut Dc) {
    let ((wx, wy), (vx, vy)) = (dc.window_ext, dc.viewport_ext);
    let (wx, wy, vx, vy) = (i64::from(wx), i64::from(wy), i64::from(vx), i64::from(vy));
    if (vx * wy).abs() > (vy * wx).abs() {
        dc.viewport_ext.0 = (vx.signum() * (vy * wx / wy).abs()) as i32;
    } else {
        dc.viewport_ext.1 = (vy.signum() * (vx * wy / wx).abs()) as i32;
    }
}

/// SetMapMode: MM_TEXT (1) .. MM_ANISOTROPIC (8); the previous mode.
/// Fixed modes set their extents; the scalable ones keep the current.
#[unsafe(no_mangle)]
pub extern "system" fn SetMapMode(hdc: Handle, mode: i32) -> i32 {
    if !(1..=8).contains(&mode) {
        return 0;
    }
    with_dc(hdc, |dc, _| {
        let prev = core::mem::replace(&mut dc.map_mode, mode);
        if mode <= 6 {
            (dc.window_ext, dc.viewport_ext) = fixed_extents(mode);
        } else if mode == 7 {
            isotropic(dc);
        }
        prev
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetMapMode(hdc: Handle) -> i32 {
    with_dc(hdc, |dc, _| dc.map_mode).unwrap_or(0)
}

fn set_extent(hdc: Handle, x: i32, y: i32, prev: *mut [i32; 2], viewport: bool) -> i32 {
    with_dc(hdc, |dc, _| {
        let e = if viewport { dc.viewport_ext } else { dc.window_ext };
        if !prev.is_null() {
            // SAFETY: the caller's SIZE.
            unsafe { *prev = [e.0, e.1] };
        }
        // Fixed modes ignore the call.
        if dc.map_mode < 7 {
            return 1;
        }
        if x == 0 || y == 0 {
            return 0;
        }
        if viewport {
            dc.viewport_ext = (x, y);
        } else {
            dc.window_ext = (x, y);
        }
        if dc.map_mode == 7 {
            isotropic(dc);
        }
        1
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn SetWindowExtEx(hdc: Handle, x: i32, y: i32, prev: *mut [i32; 2]) -> i32 {
    set_extent(hdc, x, y, prev, false)
}

#[unsafe(no_mangle)]
pub extern "system" fn SetViewportExtEx(hdc: Handle, x: i32, y: i32, prev: *mut [i32; 2]) -> i32 {
    set_extent(hdc, x, y, prev, true)
}

fn get_extent(hdc: Handle, out: *mut [i32; 2], viewport: bool) -> i32 {
    if out.is_null() {
        return 0;
    }
    with_dc(hdc, |dc, _| {
        let e = if viewport { dc.viewport_ext } else { dc.window_ext };
        // SAFETY: the caller's SIZE.
        unsafe { *out = [e.0, e.1] };
        1
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetWindowExtEx(hdc: Handle, out: *mut [i32; 2]) -> i32 {
    get_extent(hdc, out, false)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetViewportExtEx(hdc: Handle, out: *mut [i32; 2]) -> i32 {
    get_extent(hdc, out, true)
}

fn convert(hdc: Handle, points: *mut [i32; 2], n: i32, logical_to_device: bool) -> i32 {
    if n < 0 || (points.is_null() && n > 0) {
        return 0;
    }
    with_dc(hdc, |dc, _| {
        for i in 0..n as usize {
            // SAFETY: the caller's `n` POINTs.
            let p = unsafe { &mut *points.add(i) };
            let (x, y) = if logical_to_device {
                dc.logical_to_device(p[0], p[1])
            } else {
                dc.device_to_logical(p[0], p[1])
            };
            *p = [x, y];
        }
        1
    })
    .unwrap_or(0)
}

/// LPtoDP: logical points to device points, in place.
#[unsafe(no_mangle)]
pub extern "system" fn LPtoDP(hdc: Handle, points: *mut [i32; 2], n: i32) -> i32 {
    convert(hdc, points, n, true)
}

/// DPtoLP: device points to logical points, in place.
#[unsafe(no_mangle)]
pub extern "system" fn DPtoLP(hdc: Handle, points: *mut [i32; 2], n: i32) -> i32 {
    convert(hdc, points, n, false)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Each DC slot's accumulated bounds (SetBoundsRect, GetBoundsRect).
static BOUNDS: [surface::Bounds; SLOTS] = [const { surface::Bounds::empty() }; SLOTS];

const DCB_RESET: u32 = 0x1;
const DCB_ACCUMULATE: u32 = 0x2;
const DCB_SET: u32 = DCB_RESET | DCB_ACCUMULATE;
const DCB_ENABLE: u32 = 0x4;
const DCB_DISABLE: u32 = 0x8;

/// The DC's accumulation state (DCB_ENABLE or DCB_DISABLE) and its bounds
/// table entry.
fn bounds_of(hdc: Handle) -> Option<(u32, &'static surface::Bounds)> {
    let i = slot_of(hdc)?;
    let on = with_dc(hdc, |dc, _| !dc.surface.bounds.is_null())?;
    Some((if on { DCB_ENABLE } else { DCB_DISABLE }, &BOUNDS[i]))
}

/// SetBoundsRect(hdc, lprcBounds, flags): DCB_RESET clears the bounds,
/// DCB_ACCUMULATE adds the rectangle (logical), DCB_ENABLE and
/// DCB_DISABLE turn accumulation of what the DC draws on and off. The
/// previous state (DCB_SET or DCB_RESET, with DCB_ENABLE or DCB_DISABLE);
/// 0 for a handle that is not a DC.
#[unsafe(no_mangle)]
pub extern "system" fn SetBoundsRect(hdc: Handle, rect: *const [i32; 4], flags: u32) -> u32 {
    let Some((state, bounds)) = bounds_of(hdc) else { return 0 };
    let before = state | if bounds.get().is_some() { DCB_SET } else { DCB_RESET };
    if flags & DCB_RESET != 0 {
        bounds.reset();
    }
    if flags & DCB_ACCUMULATE != 0 && !rect.is_null() {
        // SAFETY: the caller's RECT.
        let [l, t, r, b] = unsafe { *rect };
        let (dx, dy) = with_dc(hdc, |dc, _| {
            (dc.viewport_org.0 - dc.window_org.0, dc.viewport_org.1 - dc.window_org.1)
        })
        .unwrap_or((0, 0));
        bounds.add(l + dx, t + dy, r + dx, b + dy);
    }
    if flags & (DCB_ENABLE | DCB_DISABLE) != 0 {
        let on = flags & DCB_ENABLE != 0;
        with_dc(hdc, |dc, _| {
            dc.surface.bounds = if on { bounds } else { core::ptr::null() };
        });
    }
    before
}

/// GetBoundsRect(hdc, lprcBounds, flags): the accumulated bounds in
/// logical coordinates (DCB_SET) or none (DCB_RESET, the rectangle
/// zeroed), without the accumulation state (Windows 11 25H2 measured
/// DCB_RESET alone with accumulation on; SetBoundsRect reports the
/// state); DCB_RESET in flags clears them
/// afterwards. Device coordinates are the DC's surface's (gala's window
/// DCs have no other mapping than the origins).
#[unsafe(no_mangle)]
pub extern "system" fn GetBoundsRect(hdc: Handle, rect: *mut [i32; 4], flags: u32) -> u32 {
    let Some((_, bounds)) = bounds_of(hdc) else { return 0 };
    let got = bounds.get();
    if !rect.is_null() {
        let (dx, dy) = with_dc(hdc, |dc, _| {
            (dc.viewport_org.0 - dc.window_org.0, dc.viewport_org.1 - dc.window_org.1)
        })
        .unwrap_or((0, 0));
        // SAFETY: the caller's RECT.
        unsafe {
            *rect = match got {
                Some([l, t, r, b]) => [l - dx, t - dy, r - dx, b - dy],
                None => [0; 4],
            };
        }
    }
    if flags & DCB_RESET != 0 {
        bounds.reset();
    }
    if got.is_some() { DCB_SET } else { DCB_RESET }
}
