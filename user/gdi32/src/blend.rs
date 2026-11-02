//! What msimg32.dll offers, here as gdi32's own exports (msimg32 forwards
//! to them, as on Windows): GdiAlphaBlend, GdiTransparentBlt and
//! GdiGradientFill (rectangles), by the documented formulas.

use crate::draw::source;
use crate::{Dc, Handle, unsupported, with_dc};

const AC_SRC_ALPHA: u32 = 0x01;

fn channel(c: u32, k: u32) -> u32 {
    (c >> (8 * k)) & 0xff
}

/// The source rectangle (logical), in device pixels, if it lies within the
/// source surface (AlphaBlend and TransparentBlt fail otherwise).
fn source_rect(s: &Dc, sx: i32, sy: i32, sw: i32, sh: i32) -> Option<(i32, i32)> {
    let (x0, y0) = s.device_point(sx, sy);
    let inside = x0 >= 0
        && y0 >= 0
        && x0.checked_add(sw).is_some_and(|r| r <= s.surface.width)
        && y0.checked_add(sh).is_some_and(|b| b <= s.surface.height);
    inside.then_some((x0, y0))
}

/// Every destination pixel of (x, y, w, h) with the source pixel (alpha in
/// the top byte) the stretch maps to it: `f(source, destination)` gives
/// the new destination pixel (alpha included) or None to leave it.
#[allow(clippy::too_many_arguments)]
fn blit(
    dst: Handle,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    src: Handle,
    sx: i32,
    sy: i32,
    sw: i32,
    sh: i32,
    f: impl Fn(u32, u32) -> Option<u32>,
) -> i32 {
    if w < 0 || h < 0 || sw < 0 || sh < 0 {
        return 0;
    }
    let Some(s) = source(src) else { return 0 };
    let Some((ssx, ssy)) = source_rect(&s, sx, sy, sw, sh) else { return 0 };
    if w == 0 || h == 0 || sw == 0 || sh == 0 {
        return 1;
    }
    with_dc(dst, |dc, _| {
        let (dx0, dy0) = dc.device_point(x, y);
        for j in 0..h {
            for i in 0..w {
                let (tx, ty) = (dx0 + i, dy0 + j);
                if !dc.visible(tx, ty) {
                    continue;
                }
                let sv = s.surface.get_raw(ssx + i * sw / w, ssy + j * sh / h).unwrap_or(0);
                let d = dc.surface.get_raw(tx, ty).unwrap_or(0);
                if let Some(v) = f(sv, d) {
                    dc.surface.put_raw(tx, ty, v);
                }
            }
        }
        1
    })
    .unwrap_or(0)
}

/// GdiAlphaBlend(dst, x, y, w, h, src, sx, sy, sw, sh, blend): BLENDFUNCTION
/// {BlendOp, BlendFlags, SourceConstantAlpha, AlphaFormat} packed in a
/// DWORD. The source is scaled by the constant alpha; with AC_SRC_ALPHA its
/// (premultiplied) alpha weighs the destination: Dst = Src + (1 - SrcA) *
/// Dst, otherwise Dst = Src + (1 - SCA) * Dst.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn GdiAlphaBlend(
    dst: Handle,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    src: Handle,
    sx: i32,
    sy: i32,
    sw: i32,
    sh: i32,
    blend: u32,
) -> i32 {
    let sca = (blend >> 16) & 0xff;
    let per_pixel = (blend >> 24) & AC_SRC_ALPHA != 0;
    let scale = |v: u32, by: u32| (v * by + 127) / 255;
    blit(dst, x, y, w, h, src, sx, sy, sw, sh, |s, d| {
        let sa = if per_pixel { scale(channel(s, 3), sca) } else { sca };
        let mut out = 0;
        for k in 0..4 {
            let sv = if k == 3 { sa } else { scale(channel(s, k), sca) };
            let v = sv + scale(channel(d, k), 255 - sa);
            out |= v.min(255) << (8 * k);
        }
        Some(out)
    })
}

/// GdiTransparentBlt(dst, x, y, w, h, src, sx, sy, sw, sh, key): the
/// (stretched) source, leaving out pixels of the key colour (COLORREF).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn GdiTransparentBlt(
    dst: Handle,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    src: Handle,
    sx: i32,
    sy: i32,
    sw: i32,
    sh: i32,
    key: u32,
) -> i32 {
    // COLORREF is 0x00BBGGRR; pixels are 0x00RRGGBB.
    let key = (key & 0xff) << 16 | (key & 0xff00) | (key >> 16) & 0xff;
    blit(dst, x, y, w, h, src, sx, sy, sw, sh, |s, d| {
        (s & 0x00ff_ffff != key).then_some(s & 0x00ff_ffff | d & 0xff00_0000)
    })
}

/// A TRIVERTEX: x, y, then red, green, blue and alpha as COLOR16 (8.8).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TriVertex {
    pub x: i32,
    pub y: i32,
    pub c: [u16; 4],
}

const GRADIENT_FILL_RECT_H: u32 = 0;
const GRADIENT_FILL_RECT_V: u32 = 1;

/// GdiGradientFill(dc, vertices, n, mesh, count, mode): GRADIENT_FILL_RECT_H
/// and _V shade each GRADIENT_RECT (two vertex indices) from its upper-left
/// vertex's colour to its lower-right's, across or down. Triangles are not
/// done yet (FALSE, in the ledger).
#[unsafe(no_mangle)]
pub extern "system" fn GdiGradientFill(
    hdc: Handle,
    vertices: *const TriVertex,
    n: u32,
    mesh: *const [u32; 2],
    count: u32,
    mode: u32,
) -> i32 {
    if mode != GRADIENT_FILL_RECT_H && mode != GRADIENT_FILL_RECT_V {
        unsupported(b"gdi32.dll!GdiGradientFill(triangles)", u64::from(mode));
        return 0;
    }
    if vertices.is_null() || mesh.is_null() {
        return 0;
    }
    with_dc(hdc, |dc, _| {
        for r in 0..count as usize {
            // SAFETY: the caller's `count` GRADIENT_RECTs.
            let [a, b] = unsafe { *mesh.add(r) };
            if a >= n || b >= n {
                return 0;
            }
            // SAFETY: indices checked against the caller's `n` vertices.
            let (va, vb) = unsafe { (*vertices.add(a as usize), *vertices.add(b as usize)) };
            let (p0, p1) = (dc.device_point(va.x, va.y), dc.device_point(vb.x, vb.y));
            // The upper-left vertex's colour is the start.
            let (from, to) = if (p0.0, p0.1) <= (p1.0, p1.1) { (va, vb) } else { (vb, va) };
            let (l, rr) = (p0.0.min(p1.0), p0.0.max(p1.0));
            let (t, bb) = (p0.1.min(p1.1), p0.1.max(p1.1));
            let span = if mode == GRADIENT_FILL_RECT_H { rr - l } else { bb - t };
            for yy in t..bb {
                for xx in l..rr {
                    if !dc.visible(xx, yy) {
                        continue;
                    }
                    let at = if mode == GRADIENT_FILL_RECT_H { xx - l } else { yy - t };
                    let mut c = 0u32;
                    for (k, shift) in [(0, 16), (1, 8), (2, 0), (3, 24)] {
                        let (f, e) = (i64::from(from.c[k]), i64::from(to.c[k]));
                        let v = f + (e - f) * i64::from(at) / i64::from(span.max(1));
                        c |= ((v >> 8) as u32 & 0xff) << shift;
                    }
                    dc.surface.put_raw(xx, yy, c);
                }
            }
        }
        1
    })
    .unwrap_or(0)
}
