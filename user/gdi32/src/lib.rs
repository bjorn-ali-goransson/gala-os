//! gala's gdi32.dll: device contexts that draw into pixel memory (a
//! window's client area, which user32 hands over with
//! `GalaCreateSurfaceDC` and presents to the desktop; a bitmap selected
//! into a memory DC; nothing for the display DC, which answers questions:
//! device caps, text metrics), the objects (brushes, pens, fonts kept as
//! LOGFONTW, bitmaps: 32-bit device-dependent ones and 24/32-bit DIB
//! sections), drawing (PatBlt, Rectangle, pixels, BitBlt/StretchBlt with
//! any ternary raster operation) and text in Noto Sans Mono scaled to the
//! font's height (`font`). Coordinates go through the window and viewport
//! origins and are clipped to the DC's clip rectangle. Pixels are
//! 0x00RRGGBB; COLORREFs 0x00BBGGRR. Objects live in a fixed table; a
//! handle is 0x0100_0000 plus the slot (stock objects keep theirs).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod dib;
mod draw;
mod font;
mod palette;
mod surface;
mod unimplemented;

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, Ordering};
use font::LogFont;
use surface::Surface;

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaScreenInfo(info: *mut [i32; 8]) -> u32;
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn VirtualAlloc(at: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    fn VirtualFree(at: *mut c_void, size: usize, kind: u32) -> i32;
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
    pub clip: Option<[i32; 4]>,
    /// MoveToEx's position (logical).
    pub pos: (i32, i32),
    pub align: u32,
    /// A child window's DC: its offset in the top-level window's surface
    /// (device = logical - window origin + viewport origin + base) and the
    /// part of the surface it may draw on (its visible rectangle).
    pub base: (i32, i32),
    pub bounds: [i32; 4],
    /// The selected palette (slot).
    pub palette: usize,
}

impl Dc {
    pub fn device_point(&self, x: i32, y: i32) -> (i32, i32) {
        (
            x - self.window_org.0 + self.viewport_org.0 + self.base.0,
            y - self.window_org.1 + self.viewport_org.1 + self.base.1,
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
            && self.clip.is_none_or(|c| x >= c[0] && y >= c[1] && x < c[2] && y < c[3])
    }

    /// Fills a device rectangle (clipped).
    pub fn fill(&self, x: i32, y: i32, w: i32, h: i32, c: u32) {
        let mut r = [
            x.max(0),
            y.max(0),
            x.saturating_add(w).min(self.surface.width),
            y.saturating_add(h).min(self.surface.height),
        ];
        if let Some(clip) = self.clip {
            r = [r[0].max(clip[0]), r[1].max(clip[1]), r[2].min(clip[2]), r[3].min(clip[3])];
        }
        let b = self.bounds;
        r = [r[0].max(b[0]), r[1].max(b[1]), r[2].min(b[2]), r[3].min(b[3])];
        for yy in r[1]..r[3] {
            for xx in r[0]..r[2] {
                self.surface.put(xx, yy, c);
            }
        }
    }
}

/// `repr(u8)` with Free first: an unused slot is all zeros, so the table
/// goes in .bss instead of the DLL's data.
#[derive(Clone, Copy)]
#[repr(u8)]
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
                bottom_up: false,
                mono: true,
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
                    Object::Bitmap(Bitmap { surface: one, bpp: 1, dib: false, owned: false }),
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
    new_object(Object::Dc(Dc {
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
    }))
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
        bottom_up: false,
        mono: false,
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
        if let Object::Bitmap(b) = slots[i] {
            if slots
                .iter()
                .any(|o| matches!(o, Object::Dc(d) if d.kind == DcKind::Memory && d.bitmap == i))
            {
                return None;
            }
            slots[i] = Object::Free;
            return Some(b.owned.then_some(b.surface.bits));
        }
        slots[i] = Object::Free;
        Some(None)
    });
    match freed {
        None => 0,
        Some(memory) => {
            if let Some(bits) = memory {
                // SAFETY: the bitmap's own memory, no longer referenced.
                unsafe { VirtualFree(bits.cast(), 0, 0x8000) };
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
                dc.surface = b.surface;
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
        Object::Pen { .. } => 1,
        Object::Brush { .. } => 2,
        Object::Dc(d) if d.kind == DcKind::Memory => 10,
        Object::Dc(_) => 3,
        Object::Font(_) => 6,
        Object::Bitmap(_) => 7,
        Object::Palette(_) => 5,
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
    let Some(i) = slot_of(h) else { return 0 };
    let object = with(|slots, _| slots[i]);
    let mut buf = [0u8; 104];
    let put = |b: &mut [u8; 104], at: usize, v: &[u8]| b[at..at + v.len()].copy_from_slice(v);
    match object {
        Object::Brush { color, null } => {
            put(&mut buf, 0, &u32::from(null).to_le_bytes()); // BS_SOLID 0, BS_NULL 1
            put(&mut buf, 4, &pixel(color).to_le_bytes());
            give(out, size, &buf[..16])
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
            let bits = if b.dib { s.bits as u64 } else { 0 };
            put(&mut buf, 24, &bits.to_le_bytes());
            if b.dib && (out.is_null() || size >= 104) {
                // The BITMAPINFOHEADER (bottom-up: positive height).
                put(&mut buf, 32, &40u32.to_le_bytes());
                put(&mut buf, 36, &s.width.to_le_bytes());
                let h = if s.bottom_up { s.height } else { -s.height };
                put(&mut buf, 40, &h.to_le_bytes());
                put(&mut buf, 44, &1u16.to_le_bytes());
                put(&mut buf, 46, &b.bpp.to_le_bytes());
                put(&mut buf, 52, &((s.stride * s.height) as u32).to_le_bytes());
                return give(out, size, &buf[..104]);
            }
            give(out, size, &buf[..32])
        }
        Object::Palette(p) => {
            let count = if p == usize::MAX { 20u16 } else { palette::count(p) };
            give(out, size, &count.to_le_bytes())
        }
        Object::Dc(_) | Object::Free => 0,
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

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
