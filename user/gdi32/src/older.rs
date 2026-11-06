//! Older GDI calls Delphi's VCL makes (Inno Setup's Setup), each the
//! documented equivalent of a newer one gala has: GetTextExtentPointW
//! (GetTextExtentPoint32W: the documentation keeps the old one for
//! 16-bit compatibility), CreatePenIndirect (CreatePen from a LOGPEN),
//! UnrealizeObject (a brush's origin and a palette's realization are
//! nothing gala keeps: TRUE for those objects), RectVisible (the
//! rectangle meets the DC's clipping region), and LineDDA (the points of
//! a line, its last one excluded, to a callback).

use crate::region::{GetClipBox, Rect};
use crate::{CreatePen, GetObjectType, Handle, draw};

/// GetTextExtentPointW: GetTextExtentPoint32W's result (the documentation
/// says the two compute the same extent for the text; only the older one
/// had a 16-bit rounding quirk, which Windows no longer has).
#[unsafe(no_mangle)]
pub extern "system" fn GetTextExtentPointW(
    hdc: Handle,
    text: *const u16,
    n: i32,
    size: *mut [i32; 2],
) -> i32 {
    draw::GetTextExtentPoint32W(hdc, text, n, size)
}

/// CreatePenIndirect(LOGPEN*): CreatePen(lopnStyle, lopnWidth.x,
/// lopnColor); NULL for no LOGPEN.
#[unsafe(no_mangle)]
pub extern "system" fn CreatePenIndirect(pen: *const u32) -> Handle {
    if pen.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: the caller's LOGPEN: style, POINT width, colour (16 bytes).
    let (style, width, color) = unsafe { (*pen, *pen.add(1), *pen.add(3)) };
    CreatePen(style as i32, width as i32, color)
}

/// UnrealizeObject(h): TRUE for a brush or a palette (nothing to reset:
/// gala aligns brushes to the DC's brush origin when drawing and maps
/// palettes on each use), FALSE for anything else.
#[unsafe(no_mangle)]
pub extern "system" fn UnrealizeObject(h: Handle) -> i32 {
    // OBJ_BRUSH, OBJ_PAL.
    i32::from(matches!(GetObjectType(h), 2 | 5))
}

/// RectVisible(hdc, rect): whether any of the rectangle (logical
/// coordinates) is within the DC's clipping region's bounds.
#[unsafe(no_mangle)]
pub extern "system" fn RectVisible(hdc: Handle, rect: *const Rect) -> i32 {
    if rect.is_null() {
        return 0;
    }
    let mut clip: Rect = [0; 4];
    // 0: ERROR (no DC), 1: NULLREGION.
    if matches!(GetClipBox(hdc, &mut clip), 0 | 1) {
        return 0;
    }
    // SAFETY: the caller's RECT.
    let r = unsafe { *rect };
    let (l, t, rr, b) = (r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3]));
    i32::from(l < clip[2] && rr > clip[0] && t < clip[3] && b > clip[1])
}

type LineDdaProc = extern "system" fn(i32, i32, isize);

/// LineDDA(x1, y1, x2, y2, proc, data): calls `proc` for each point of
/// the line from (x1, y1) to (x2, y2) except the last: one point per step
/// along the longer axis, the other coordinate rounded to the nearest
/// (halves away from the start). TRUE (FALSE without a procedure).
#[unsafe(no_mangle)]
pub extern "system" fn LineDDA(
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    proc_: Option<LineDdaProc>,
    data: isize,
) -> i32 {
    let Some(proc_) = proc_ else { return 0 };
    for (x, y) in line_points(x1, y1, x2, y2) {
        proc_(x, y, data);
    }
    1
}

/// The points LineDDA reports, the last one excluded.
fn line_points(x1: i32, y1: i32, x2: i32, y2: i32) -> impl Iterator<Item = (i32, i32)> {
    let (dx, dy) = (i64::from(x2) - i64::from(x1), i64::from(y2) - i64::from(y1));
    let steps = dx.abs().max(dy.abs());
    (0..steps).map(move |i| {
        // The minor coordinate: start + delta * i / steps, rounded half
        // away from the start.
        let along = |d: i64| {
            let num = d * i * 2;
            let q = (num.abs() + steps) / (2 * steps);
            if num < 0 { -q } else { q }
        };
        ((i64::from(x1) + along(dx)) as i32, (i64::from(y1) + along(dy)) as i32)
    })
}
