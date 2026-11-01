//! gala's gdi32.dll: device contexts that draw into a pixel buffer (a
//! window's client area, which user32 hands over with
//! `GalaCreateSurfaceDC` and presents to the desktop), solid brushes and
//! the stock objects, PatBlt and text (TextOutW/A, GetTextExtentPoint32W)
//! in one fixed-pitch font, the 16-pixel bitmap font the kernel console
//! uses (noto-sans-mono-bitmap). Pixels are 0x00RRGGBB; COLORREFs
//! 0x00BBGGRR. Objects live in a fixed table; a handle is 0x0100_0000 plus
//! the slot (stock objects keep theirs for the process's life).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, Ordering};
use noto_sans_mono_bitmap::{FontWeight, RasterHeight, get_raster, get_raster_width};

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

type Handle = *mut c_void;

const FONT: FontWeight = FontWeight::Regular;
const SIZE: RasterHeight = RasterHeight::Size16;
const SLOTS: usize = 512;
const BASE: usize = 0x0100_0000;

#[derive(Clone, Copy)]
enum Object {
    Free,
    Brush { color: u32, null: bool },
    Pen { color: u32 },
    Font,
    Dc(Dc),
}

#[derive(Clone, Copy)]
struct Dc {
    pixels: *mut u32,
    width: i32,
    height: i32,
    /// Pixels per row.
    stride: i32,
    text: u32,
    background: u32,
    /// TRANSPARENT (1) or OPAQUE (2).
    mode: i32,
    brush: usize,
    pen: usize,
    font: usize,
}

struct Table {
    lock: AtomicBool,
    slots: UnsafeCell<[Object; SLOTS]>,
    stock: UnsafeCell<[usize; 20]>,
    ready: UnsafeCell<bool>,
}

// SAFETY: every access is under `lock`.
unsafe impl Sync for Table {}

static TABLE: Table = Table {
    lock: AtomicBool::new(false),
    slots: UnsafeCell::new([Object::Free; SLOTS]),
    stock: UnsafeCell::new([0; 20]),
    ready: UnsafeCell::new(false),
};

/// Runs `f` on the object table (with the stock objects made).
fn with<R>(f: impl FnOnce(&mut [Object; SLOTS], &[usize; 20]) -> R) -> R {
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = unsafe {
        let slots = &mut *TABLE.slots.get();
        let stock = &mut *TABLE.stock.get();
        if !*TABLE.ready.get() {
            // WHITE, LTGRAY, GRAY, DKGRAY, BLACK, NULL brushes; WHITE,
            // BLACK, NULL pens; the fonts (one face for all).
            let objects = [
                (0, Object::Brush { color: 0xffffff, null: false }),
                (1, Object::Brush { color: 0xc0c0c0, null: false }),
                (2, Object::Brush { color: 0x808080, null: false }),
                (3, Object::Brush { color: 0x404040, null: false }),
                (4, Object::Brush { color: 0x000000, null: false }),
                (5, Object::Brush { color: 0, null: true }),
                (6, Object::Pen { color: 0xffffff }),
                (7, Object::Pen { color: 0x000000 }),
                (8, Object::Pen { color: 0 }),
                (10, Object::Font),
                (11, Object::Font),
                (12, Object::Font),
                (13, Object::Font),
                (14, Object::Font),
                (16, Object::Font),
                (17, Object::Font),
            ];
            for (slot, (index, object)) in objects.into_iter().enumerate() {
                slots[slot] = object;
                stock[index] = slot;
            }
            *TABLE.ready.get() = true;
        }
        f(slots, stock)
    };
    TABLE.lock.store(false, Ordering::Release);
    r
}

fn handle(slot: usize) -> Handle {
    (BASE + slot) as Handle
}

fn slot_of(h: Handle) -> Option<usize> {
    let v = (h as usize).checked_sub(BASE)?;
    (v < SLOTS).then_some(v)
}

fn new_object(object: Object) -> Handle {
    with(|slots, _| match slots.iter().position(|o| matches!(o, Object::Free)) {
        Some(i) => {
            slots[i] = object;
            handle(i)
        }
        None => core::ptr::null_mut(),
    })
}

/// COLORREF (0x00BBGGRR) to a pixel (0x00RRGGBB).
fn pixel(color: u32) -> u32 {
    (color & 0xff) << 16 | (color & 0xff00) | (color >> 16) & 0xff
}

/// Runs `f` on DC `h`'s state, if it is one.
fn with_dc<R>(h: Handle, f: impl FnOnce(&mut Dc, &mut [Object; SLOTS]) -> R) -> Option<R> {
    let i = slot_of(h)?;
    with(|slots, _| {
        let Object::Dc(mut dc) = slots[i] else { return None };
        let r = f(&mut dc, slots);
        slots[i] = Object::Dc(dc);
        Some(r)
    })
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
    let (brush, pen, font) = with(|_, stock| (stock[0], stock[7], stock[13]));
    new_object(Object::Dc(Dc {
        pixels,
        width,
        height,
        stride,
        text: 0,
        background: 0xffffff,
        mode: 2,
        brush,
        pen,
        font,
    }))
}

#[unsafe(no_mangle)]
pub extern "system" fn GalaDeleteSurfaceDC(hdc: Handle) -> i32 {
    DeleteObject(hdc)
}

// ---- Objects ----

#[unsafe(no_mangle)]
pub extern "system" fn GetStockObject(index: i32) -> Handle {
    if !(0..20).contains(&index) {
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

/// CreatePen(style, width, color): a solid pen (style and width are kept
/// for later; lines are one pixel).
#[unsafe(no_mangle)]
pub extern "system" fn CreatePen(_style: i32, _width: i32, color: u32) -> Handle {
    new_object(Object::Pen { color: pixel(color) })
}

/// DeleteObject: frees a brush, pen, font or DC (stock objects stay).
#[unsafe(no_mangle)]
pub extern "system" fn DeleteObject(h: Handle) -> i32 {
    let Some(i) = slot_of(h) else { return 0 };
    with(|slots, stock| {
        if matches!(slots[i], Object::Free) {
            return 0;
        }
        if !stock.contains(&i) {
            slots[i] = Object::Free;
        }
        1
    })
}

/// SelectObject(hdc, object): the previously selected object of its kind.
#[unsafe(no_mangle)]
pub extern "system" fn SelectObject(hdc: Handle, object: Handle) -> Handle {
    let Some(new) = slot_of(object) else { return core::ptr::null_mut() };
    with_dc(hdc, |dc, slots| {
        let field = match slots[new] {
            Object::Brush { .. } => &mut dc.brush,
            Object::Pen { .. } => &mut dc.pen,
            Object::Font => &mut dc.font,
            _ => return core::ptr::null_mut(),
        };
        handle(core::mem::replace(field, new))
    })
    .unwrap_or(core::ptr::null_mut())
}

/// The COLORREF of a pixel.
fn colorref(p: u32) -> u32 {
    pixel(p) // the swap is its own inverse
}

#[unsafe(no_mangle)]
pub extern "system" fn SetTextColor(hdc: Handle, color: u32) -> u32 {
    with_dc(hdc, |dc, _| colorref(core::mem::replace(&mut dc.text, pixel(color))))
        .unwrap_or(0xffff_ffff)
}

#[unsafe(no_mangle)]
pub extern "system" fn SetBkColor(hdc: Handle, color: u32) -> u32 {
    with_dc(hdc, |dc, _| colorref(core::mem::replace(&mut dc.background, pixel(color))))
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
    with_dc(hdc, |dc, _| colorref(dc.text)).unwrap_or(0xffff_ffff)
}

// ---- Drawing ----

/// Fills the rectangle (clipped) with `color`.
fn fill(dc: &Dc, x: i32, y: i32, w: i32, h: i32, color: u32) {
    let (x0, y0) = (x.max(0), y.max(0));
    let (x1, y1) = (x.saturating_add(w).min(dc.width), y.saturating_add(h).min(dc.height));
    for yy in y0..y1 {
        for xx in x0..x1 {
            // SAFETY: inside the surface (clipped above).
            unsafe { *dc.pixels.add((yy * dc.stride + xx) as usize) = color };
        }
    }
}

const PATCOPY: u32 = 0x00f0_0021;
const BLACKNESS: u32 = 0x0000_0042;
const WHITENESS: u32 = 0x00ff_0062;

/// PatBlt(hdc, x, y, w, h, rop): PATCOPY (the selected brush),
/// BLACKNESS, WHITENESS.
#[unsafe(no_mangle)]
pub extern "system" fn PatBlt(hdc: Handle, x: i32, y: i32, w: i32, h: i32, rop: u32) -> i32 {
    with_dc(hdc, |dc, slots| {
        let color = match rop {
            BLACKNESS => 0,
            WHITENESS => 0xffffff,
            PATCOPY => match slots[dc.brush] {
                Object::Brush { null: true, .. } => return 1,
                Object::Brush { color, .. } => color,
                _ => return 0,
            },
            _ => return 0,
        };
        let (x, w) = if w < 0 { (x + w, -w) } else { (x, w) };
        let (y, h) = if h < 0 { (y + h, -h) } else { (y, h) };
        fill(dc, x, y, w, h, color);
        1
    })
    .unwrap_or(0)
}

/// Rectangle(hdc, left, top, right, bottom): the brush inside, the pen's
/// one-pixel outline.
#[unsafe(no_mangle)]
pub extern "system" fn Rectangle(hdc: Handle, l: i32, t: i32, r: i32, b: i32) -> i32 {
    with_dc(hdc, |dc, slots| {
        if let Object::Brush { color, null: false } = slots[dc.brush] {
            fill(dc, l, t, r - l, b - t, color);
        }
        if let Object::Pen { color } = slots[dc.pen]
            && dc.pen != with_null_pen(slots)
        {
            fill(dc, l, t, r - l, 1, color);
            fill(dc, l, b - 1, r - l, 1, color);
            fill(dc, l, t, 1, b - t, color);
            fill(dc, r - 1, t, 1, b - t, color);
        }
        1
    })
    .unwrap_or(0)
}

/// The NULL_PEN's slot (stock 8).
fn with_null_pen(slots: &[Object; SLOTS]) -> usize {
    let _ = slots;
    // SAFETY: called under the table lock (from `with`), where the stock
    // table is set.
    unsafe { (*TABLE.stock.get())[8] }
}

fn cell() -> (i32, i32) {
    (get_raster_width(FONT, SIZE) as i32, SIZE.val() as i32)
}

/// Draws `text` at (x, y): each glyph's coverage blends the text colour
/// over the surface; OPAQUE fills the cells with the background first.
fn draw_text(dc: &Dc, x: i32, y: i32, text: impl Iterator<Item = char>) {
    let (cw, ch) = cell();
    for (i, c) in text.enumerate() {
        let gx = x + i as i32 * cw;
        if dc.mode == 2 {
            fill(dc, gx, y, cw, ch, dc.background);
        }
        let Some(raster) = get_raster(c, FONT, SIZE).or_else(|| get_raster('?', FONT, SIZE)) else {
            continue;
        };
        for (dy, row) in raster.raster().iter().enumerate() {
            for (dx, &t) in row.iter().enumerate() {
                let (px, py) = (gx + dx as i32, y + dy as i32);
                if t == 0 || px < 0 || py < 0 || px >= dc.width || py >= dc.height {
                    continue;
                }
                // SAFETY: inside the surface (checked).
                unsafe {
                    let p = dc.pixels.add((py * dc.stride + px) as usize);
                    *p = blend(*p, dc.text, t);
                }
            }
        }
    }
}

fn blend(bg: u32, fg: u32, t: u8) -> u32 {
    let t = u32::from(t);
    let mix = |shift: u32| {
        let (a, b) = ((bg >> shift) & 0xff, (fg >> shift) & 0xff);
        ((a * (255 - t) + b * t) / 255) << shift
    };
    mix(16) | mix(8) | mix(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn TextOutW(hdc: Handle, x: i32, y: i32, text: *const u16, n: i32) -> i32 {
    if n < 0 || (text.is_null() && n > 0) {
        return 0;
    }
    // SAFETY: the caller's `n` characters.
    let s = if n == 0 { &[][..] } else { unsafe { core::slice::from_raw_parts(text, n as usize) } };
    with_dc(hdc, |dc, _| {
        draw_text(dc, x, y, char::decode_utf16(s.iter().copied()).map(|c| c.unwrap_or('?')));
        1
    })
    .unwrap_or(0)
}

/// TextOutA: the ANSI code page is UTF-8 here.
#[unsafe(no_mangle)]
pub extern "system" fn TextOutA(hdc: Handle, x: i32, y: i32, text: *const u8, n: i32) -> i32 {
    if n < 0 || (text.is_null() && n > 0) {
        return 0;
    }
    // SAFETY: the caller's `n` bytes.
    let s = if n == 0 { &[][..] } else { unsafe { core::slice::from_raw_parts(text, n as usize) } };
    let text = core::str::from_utf8(s).unwrap_or("?");
    with_dc(hdc, |dc, _| {
        draw_text(dc, x, y, text.chars());
        1
    })
    .unwrap_or(0)
}

/// GetTextExtentPoint32W(hdc, text, n, &size): n cells of the font.
#[unsafe(no_mangle)]
pub extern "system" fn GetTextExtentPoint32W(
    hdc: Handle,
    text: *const u16,
    n: i32,
    size: *mut [i32; 2],
) -> i32 {
    if slot_of(hdc).is_none() || size.is_null() || n < 0 {
        return 0;
    }
    // SAFETY: the caller's `n` characters.
    let s = if n == 0 || text.is_null() {
        &[][..]
    } else {
        unsafe { core::slice::from_raw_parts(text, n as usize) }
    };
    let chars = char::decode_utf16(s.iter().copied()).count() as i32;
    let (cw, ch) = cell();
    // SAFETY: the caller's output.
    unsafe { *size = [chars * cw, ch] };
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
