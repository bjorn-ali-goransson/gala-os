//! Drawing primitives: pen lines through the DC's ROP2 mix, polygons
//! (ALTERNATE or WINDING fill, then the outline), polylines, ellipses and
//! rounded rectangles (filled row by row with integer square roots, the
//! outline from both row and column scans so steep parts have no gaps);
//! the modes programs set (ROP2, polygon fill, brush origin, graphics mode
//! and world transform, layout: kept, the transform and right-to-left
//! layouts not applied); ExtCreatePen; hatch and pattern brushes (drawn
//! solid); MaskBlt. Arc and Pie are recorded and draw nothing yet.

use crate::draw::{brush, pen, rop3};
use crate::{Dc, Handle, Object, unsupported, with, with_dc};

/// A pen of ExtCreatePen (OBJ_EXTPEN): this bit in its style.
pub const EXT_PEN: i32 = 0x4000_0000;

/// A pixel of the pen: `c` mixed with the destination by the DC's ROP2.
fn mix(dc: &Dc, x: i32, y: i32, c: u32) {
    if !dc.visible(x, y) {
        return;
    }
    let d = dc.surface.get(x, y).unwrap_or(0);
    let out = match dc.rop2 {
        1 => 0,            // R2_BLACK
        6 => !d,           // R2_NOT
        7 => d ^ c,        // R2_XORPEN
        9 => d & c,        // R2_MASKPEN
        10 => !(d ^ c),    // R2_NOTXORPEN
        11 => d,           // R2_NOP
        15 => d | c,       // R2_MERGEPEN
        16 => 0x00ff_ffff, // R2_WHITE
        4 => !c,           // R2_NOTCOPYPEN
        _ => c,            // R2_COPYPEN and the rest
    };
    dc.surface.put(x, y, out & 0x00ff_ffff);
}

/// A pen dot `w` wide at (x, y).
fn dot(dc: &Dc, x: i32, y: i32, c: u32, w: i32) {
    let w = w.max(1);
    for j in 0..w {
        for i in 0..w {
            mix(dc, x - (w - 1) / 2 + i, y - (w - 1) / 2 + j, c);
        }
    }
}

/// A pen line from (x0, y0) to (x1, y1), the end excluded (Bresenham),
/// in device coordinates.
pub fn line(dc: &Dc, x0: i32, y0: i32, x1: i32, y1: i32, c: u32, w: i32) {
    let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
    let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
    let (mut px, mut py, mut err) = (x0, y0, dx + dy);
    while (px, py) != (x1, y1) {
        dot(dc, px, py, c, w);
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            px += sx;
        }
        if e2 <= dx {
            err += dx;
            py += sy;
        }
    }
}

fn isqrt(v: i64) -> i64 {
    if v <= 0 {
        return 0;
    }
    let mut x = 0i64;
    let mut step = 1i64 << 31;
    while step > 0 {
        if (x + step) * (x + step) <= v {
            x += step;
        }
        step >>= 1;
    }
    x
}

/// The caller's `n` POINTs, as device points.
fn device_points<'a>(
    dc: &Dc,
    p: *const [i32; 2],
    n: usize,
    out: &'a mut [(i32, i32); 1024],
) -> &'a [(i32, i32)] {
    let n = n.min(out.len());
    for (i, slot) in out.iter_mut().enumerate().take(n) {
        // SAFETY: the caller's array of `n` POINTs.
        let [x, y] = unsafe { *p.add(i) };
        *slot = dc.device_point(x, y);
    }
    &out[..n]
}

/// Fills polygons (each closed) by the DC's fill mode: for each row, the
/// edges' crossings at its middle, filled between them (ALTERNATE: odd
/// crossings; WINDING: a nonzero count of directed crossings).
fn fill_polygons(dc: &Dc, polys: &[&[(i32, i32)]], c: u32) {
    let all = polys.iter().flat_map(|p| p.iter());
    let (top, bottom) = all.fold((i32::MAX, i32::MIN), |(t, b), &(_, y)| (t.min(y), b.max(y)));
    if top >= bottom {
        return;
    }
    let winding = dc.poly_fill == 2;
    for y in top..bottom {
        let fy = 2 * y + 1;
        let mut xs = [(0i32, 0i32); 256];
        let mut n = 0;
        for p in polys {
            for k in 0..p.len() {
                let (a, b) = (p[k], p[(k + 1) % p.len()]);
                let (ay, by) = (2 * a.1, 2 * b.1);
                if (ay <= fy) != (by <= fy) && n < xs.len() {
                    let x = a.0 + ((fy - ay) * (b.0 - a.0)) / (by - ay);
                    xs[n] = (x, if by > ay { 1 } else { -1 });
                    n += 1;
                }
            }
        }
        let xs = &mut xs[..n];
        xs.sort_unstable_by_key(|e| e.0);
        let mut count = 0;
        for k in 0..n {
            let inside_before = if winding { count != 0 } else { count % 2 != 0 };
            count += if winding { xs[k].1 } else { 1 };
            let _ = inside_before;
            let inside_after = if winding { count != 0 } else { count % 2 != 0 };
            if inside_after && k + 1 < n {
                dc.fill(xs[k].0, y, xs[k + 1].0 - xs[k].0, 1, c);
            }
        }
    }
}

fn outline(dc: &Dc, p: &[(i32, i32)], closed: bool, c: u32, w: i32) {
    for k in 0..p.len().saturating_sub(1) {
        line(dc, p[k].0, p[k].1, p[k + 1].0, p[k + 1].1, c, w);
    }
    if closed && p.len() > 2 {
        let (a, b) = (p[p.len() - 1], p[0]);
        line(dc, a.0, a.1, b.0, b.1, c, w);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Polygon(hdc: Handle, points: *const [i32; 2], n: i32) -> i32 {
    if points.is_null() || n < 2 {
        return 0;
    }
    with_dc(hdc, |dc, slots| {
        let mut buf = [(0, 0); 1024];
        let p = device_points(dc, points, n as usize, &mut buf);
        if let Some(c) = brush(dc, slots) {
            fill_polygons(dc, &[p], c);
        }
        if let Some((c, w)) = pen(dc, slots) {
            outline(dc, p, true, c, w);
        }
        1
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn PolyPolygon(
    hdc: Handle,
    points: *const [i32; 2],
    counts: *const i32,
    polys: i32,
) -> i32 {
    if points.is_null() || counts.is_null() || polys <= 0 {
        return 0;
    }
    // SAFETY: the caller's `polys` counts.
    let counts = unsafe { core::slice::from_raw_parts(counts, polys as usize) };
    with_dc(hdc, |dc, slots| {
        let mut at = 0usize;
        for &n in counts {
            let mut buf = [(0, 0); 1024];
            // SAFETY: inside the caller's points.
            let p = device_points(dc, unsafe { points.add(at) }, n.max(0) as usize, &mut buf);
            if let Some(c) = brush(dc, slots) {
                fill_polygons(dc, &[p], c);
            }
            if let Some((c, w)) = pen(dc, slots) {
                outline(dc, p, true, c, w);
            }
            at += n.max(0) as usize;
        }
        1
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn Polyline(hdc: Handle, points: *const [i32; 2], n: i32) -> i32 {
    if points.is_null() || n < 2 {
        return 0;
    }
    with_dc(hdc, |dc, slots| {
        let mut buf = [(0, 0); 1024];
        let p = device_points(dc, points, n as usize, &mut buf);
        if let Some((c, w)) = pen(dc, slots) {
            outline(dc, p, false, c, w);
        }
        1
    })
    .unwrap_or(0)
}

/// The half-width (doubled) of an ellipse `wd` x `ht` (doubled units) at
/// a row `dy` (doubled) from its centre.
fn half_width(wd: i64, ht: i64, dy: i64) -> i64 {
    if ht <= 0 || dy.abs() > ht {
        return -1;
    }
    isqrt(wd * wd * (ht * ht - dy * dy) / (ht * ht))
}

/// Fills and outlines the shape whose rows run from `inset(row)` in from
/// both sides of [l, r): an ellipse or a rounded rectangle.
#[allow(clippy::too_many_arguments)]
fn rounded(
    dc: &Dc,
    l: i32,
    t: i32,
    r: i32,
    b: i32,
    rw: i32,
    rh: i32,
    fill: Option<u32>,
    pen: Option<(u32, i32)>,
) {
    // The corner ellipse: rw x rh (the whole box for an ellipse).
    let (rw, rh) = (rw.min(r - l).max(0) as i64, rh.min(b - t).max(0) as i64);
    let inset = |y: i32| -> i32 {
        // Rows within rh/2 of the top or bottom are in a corner.
        let (yt, yb) = (y - t, b - 1 - y);
        let k = yt.min(yb) as i64;
        if 2 * k + 1 >= rh || rh == 0 {
            return 0;
        }
        let dy = rh - (2 * k + 1);
        let hw = half_width(rw, rh, dy);
        ((rw - hw) / 2) as i32
    };
    if let Some(c) = fill {
        for y in t..b {
            let i = inset(y);
            dc.fill(l + i, y, (r - l) - 2 * i, 1, c);
        }
    }
    if let Some((c, w)) = pen {
        // Rows: the two ends of each row; the top and bottom rows whole.
        for y in t..b {
            let i = inset(y);
            if y == t || y == b - 1 {
                for x in l + i..r - i {
                    dot(dc, x, y, c, w);
                }
            } else {
                dot(dc, l + i, y, c, w);
                dot(dc, r - 1 - i, y, c, w);
            }
        }
        // Columns: where the curve is flat, rows leave gaps.
        for x in l..r {
            let (xl, xr) = (x - l, r - 1 - x);
            let k = xl.min(xr) as i64;
            if 2 * k + 1 >= rw || rw == 0 {
                continue;
            }
            let dx = rw - (2 * k + 1);
            let hh = half_width(rh, rw, dx);
            let i = ((rh - hh) / 2) as i32;
            dot(dc, x, t + i, c, w);
            dot(dc, x, b - 1 - i, c, w);
        }
    }
}

fn corners(dc: &Dc, l: i32, t: i32, r: i32, b: i32) -> (i32, i32, i32, i32) {
    let (x0, y0) = dc.device_point(l, t);
    let (x1, y1) = dc.device_point(r, b);
    (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1))
}

#[unsafe(no_mangle)]
pub extern "system" fn Ellipse(hdc: Handle, l: i32, t: i32, r: i32, b: i32) -> i32 {
    with_dc(hdc, |dc, slots| {
        let (l, t, r, b) = corners(dc, l, t, r, b);
        rounded(dc, l, t, r, b, r - l, b - t, brush(dc, slots), pen(dc, slots));
        1
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn RoundRect(
    hdc: Handle,
    l: i32,
    t: i32,
    r: i32,
    b: i32,
    w: i32,
    h: i32,
) -> i32 {
    with_dc(hdc, |dc, slots| {
        let (l, t, r, b) = corners(dc, l, t, r, b);
        rounded(dc, l, t, r, b, w, h, brush(dc, slots), pen(dc, slots));
        1
    })
    .unwrap_or(0)
}

/// Arc and Pie: not drawn yet (recorded).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Arc(
    _hdc: Handle,
    _l: i32,
    _t: i32,
    _r: i32,
    _b: i32,
    _x1: i32,
    _y1: i32,
    _x2: i32,
    _y2: i32,
) -> i32 {
    unsupported(b"gdi32.dll!Arc", 0);
    1
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Pie(
    _hdc: Handle,
    _l: i32,
    _t: i32,
    _r: i32,
    _b: i32,
    _x1: i32,
    _y1: i32,
    _x2: i32,
    _y2: i32,
) -> i32 {
    unsupported(b"gdi32.dll!Pie", 0);
    1
}

// ---- Modes ----

macro_rules! mode {
    ($set:ident, $get:ident, $field:ident, $valid:expr) => {
        #[unsafe(no_mangle)]
        pub extern "system" fn $set(hdc: Handle, value: i32) -> i32 {
            if !$valid(value) {
                return 0;
            }
            with_dc(hdc, |dc, _| core::mem::replace(&mut dc.$field, value)).unwrap_or(0)
        }

        #[unsafe(no_mangle)]
        pub extern "system" fn $get(hdc: Handle) -> i32 {
            with_dc(hdc, |dc, _| dc.$field).unwrap_or(0)
        }
    };
}

mode!(SetROP2, GetROP2, rop2, |v: i32| (1..=16).contains(&v));
mode!(SetPolyFillMode, GetPolyFillMode, poly_fill, |v: i32| v == 1 || v == 2);
mode!(SetGraphicsMode, GetGraphicsMode, graphics_mode, |v: i32| v == 1 || v == 2);

#[unsafe(no_mangle)]
pub extern "system" fn SetBrushOrgEx(hdc: Handle, x: i32, y: i32, prev: *mut [i32; 2]) -> i32 {
    with_dc(hdc, |dc, _| {
        if !prev.is_null() {
            // SAFETY: the caller's POINT.
            unsafe { *prev = [dc.brush_org.0, dc.brush_org.1] };
        }
        dc.brush_org = (x, y);
        1
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetBrushOrgEx(hdc: Handle, out: *mut [i32; 2]) -> i32 {
    with_dc(hdc, |dc, _| {
        if !out.is_null() {
            // SAFETY: the caller's POINT.
            unsafe { *out = [dc.brush_org.0, dc.brush_org.1] };
        }
        1
    })
    .unwrap_or(0)
}

/// SetLayout: the previous layout; right-to-left is not applied
/// (recorded).
#[unsafe(no_mangle)]
pub extern "system" fn SetLayout(hdc: Handle, layout: u32) -> u32 {
    if layout & 1 != 0 {
        unsupported(b"gdi32.dll!SetLayout(LAYOUT_RTL)", u64::from(layout));
    }
    with_dc(hdc, |dc, _| core::mem::replace(&mut dc.layout, layout)).unwrap_or(0xffff_ffff)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetLayout(hdc: Handle) -> u32 {
    with_dc(hdc, |dc, _| dc.layout).unwrap_or(0xffff_ffff)
}

const IDENTITY: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn note_transform(x: &[f32; 6]) {
    if *x != IDENTITY {
        unsupported(b"gdi32.dll!world transform (not applied)", 0);
    }
}

/// SetWorldTransform (GM_ADVANCED only): kept; a transform other than
/// the identity is not applied to drawing (recorded).
#[unsafe(no_mangle)]
pub extern "system" fn SetWorldTransform(hdc: Handle, xform: *const [f32; 6]) -> i32 {
    if xform.is_null() {
        return 0;
    }
    // SAFETY: the caller's XFORM.
    let x = unsafe { *xform };
    with_dc(hdc, |dc, _| {
        if dc.graphics_mode != 2 {
            return 0;
        }
        note_transform(&x);
        dc.xform = x;
        1
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetWorldTransform(hdc: Handle, out: *mut [f32; 6]) -> i32 {
    if out.is_null() {
        return 0;
    }
    with_dc(hdc, |dc, _| {
        // SAFETY: the caller's XFORM.
        unsafe { *out = dc.xform };
        1
    })
    .unwrap_or(0)
}

/// a * b for XFORMs (row vectors: a first).
fn multiply(a: &[f32; 6], b: &[f32; 6]) -> [f32; 6] {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

/// ModifyWorldTransform: MWT_IDENTITY (1), MWT_LEFTMULTIPLY (2),
/// MWT_RIGHTMULTIPLY (3).
#[unsafe(no_mangle)]
pub extern "system" fn ModifyWorldTransform(hdc: Handle, xform: *const [f32; 6], mode: u32) -> i32 {
    let given = if xform.is_null() {
        IDENTITY
    } else {
        // SAFETY: the caller's XFORM.
        unsafe { *xform }
    };
    with_dc(hdc, |dc, _| {
        let new = match mode {
            1 => IDENTITY,
            2 => multiply(&given, &dc.xform),
            3 => multiply(&dc.xform, &given),
            _ => return 0,
        };
        note_transform(&new);
        dc.xform = new;
        1
    })
    .unwrap_or(0)
}

// ---- Pens and brushes ----

/// ExtCreatePen(style, width, LOGBRUSH, count, styles): a pen of the
/// brush's colour and the width (OBJ_EXTPEN); dashes and caps are drawn
/// solid.
#[unsafe(no_mangle)]
pub extern "system" fn ExtCreatePen(
    style: u32,
    width: u32,
    lb: *const [u32; 4],
    _n: u32,
    _styles: *const u32,
) -> Handle {
    if lb.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: the caller's LOGBRUSH: lbStyle, lbColor, lbHatch.
    let [_, color, _, _] = unsafe { *lb };
    let kind = (style & 0xf) as i32;
    crate::new_object(Object::Pen {
        color: crate::pixel(color),
        null: kind == 5, // PS_NULL
        width: width.max(1) as i32,
        style: kind | EXT_PEN,
    })
}

/// CreateHatchBrush: drawn as the solid colour (recorded).
#[unsafe(no_mangle)]
pub extern "system" fn CreateHatchBrush(style: i32, color: u32) -> Handle {
    unsupported(b"gdi32.dll!CreateHatchBrush (drawn solid)", style as u64);
    crate::new_object(Object::Brush { color: crate::pixel(color), null: false })
}

/// CreatePatternBrush: drawn as the bitmap's first pixel's colour
/// (recorded).
#[unsafe(no_mangle)]
pub extern "system" fn CreatePatternBrush(bitmap: Handle) -> Handle {
    unsupported(b"gdi32.dll!CreatePatternBrush (drawn solid)", 0);
    let color = crate::slot_of(bitmap)
        .and_then(|i| {
            with(|slots, _| match slots[i] {
                Object::Bitmap(b) => b.surface.get(0, 0),
                _ => None,
            })
        })
        .unwrap_or(0);
    crate::new_object(Object::Brush { color, null: false })
}

/// ExtFloodFill: not there yet (recorded; fails).
#[unsafe(no_mangle)]
pub extern "system" fn ExtFloodFill(_hdc: Handle, x: i32, y: i32, _color: u32, _kind: u32) -> i32 {
    unsupported(b"gdi32.dll!ExtFloodFill", ((x as u64) << 32) | y as u32 as u64);
    0
}

/// MaskBlt(dst, x, y, w, h, src, sx, sy, mask, mx, my, rop4): per pixel,
/// the foreground ROP3 (the low 24 bits) where the monochrome mask's bit
/// is 1, the background ROP3 (bits 24-31, as a ROP3's index) where it is
/// 0; without a mask, the foreground everywhere.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn MaskBlt(
    dst: Handle,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    src: Handle,
    sx: i32,
    sy: i32,
    mask: Handle,
    mx: i32,
    my: i32,
    rop4: u32,
) -> i32 {
    let fore = rop4 & 0x00ff_ffff;
    let back = (rop4 >> 8) & 0x00ff_0000;
    let source = crate::slot_of(src).and_then(|i| {
        with(|slots, _| match slots[i] {
            Object::Dc(d) => Some(d),
            _ => None,
        })
    });
    let mask_surface = crate::slot_of(mask).and_then(|i| {
        with(|slots, _| match slots[i] {
            Object::Bitmap(b) => Some(b.surface),
            _ => None,
        })
    });
    with_dc(dst, |dc, slots| {
        let p = brush(dc, slots).unwrap_or(0);
        let (dx0, dy0) = dc.device_point(x, y);
        for j in 0..h.max(0) {
            for i in 0..w.max(0) {
                let (px, py) = (dx0 + i, dy0 + j);
                if !dc.visible(px, py) {
                    continue;
                }
                let s = source.map_or(0, |sdc| {
                    let (qx, qy) = sdc.device_point(sx + i, sy + j);
                    sdc.surface.get(qx, qy).unwrap_or(0)
                });
                let on = mask_surface
                    .is_none_or(|m| m.get(mx + i, my + j).unwrap_or(0) & 0x00ff_ffff != 0);
                let d = dc.surface.get(px, py).unwrap_or(0);
                let rop = if on { fore } else { back };
                dc.surface.put(px, py, rop3(rop, p, s, d));
            }
        }
        1
    })
    .unwrap_or(0)
}
