//! The software rasterizer every render target draws with.
//!
//! - Figures (`geometry::Path`) are flattened to polygons in device
//!   pixels: lines as they are, Béziers subdivided until each piece is
//!   within a tenth of a pixel of the curve.
//! - Coverage: antialiased drawing samples each pixel on a 4 x 4 grid
//!   (16 levels), aliased drawing at the pixel's centre only (a pixel is
//!   in when its centre is: D2D1_ANTIALIAS_MODE_ALIASED). A sample is in
//!   by the fill rule over every polygon's edges (alternate: odd
//!   crossings; winding: a non-zero sum).
//! - Strokes become polygons: one rectangle per segment (flat caps), the
//!   joins (miter within the limit, else bevel; round as a circle) and
//!   the caps, each turned to the same orientation so the winding rule
//!   unites them.
//! - Clips: the intersection of the pushed axis-aligned rectangles, in
//!   device pixels; an aliased clip is rounded to whole pixels, an
//!   antialiased one scales the coverage of the pixels it cuts.
//! - Paint: a premultiplied colour, a gradient (linear or radial, its
//!   stops in premultiplied form, clamp/wrap/mirror) or a bitmap (nearest
//!   or bilinear), each sampled at the pixel's centre through the inverse
//!   of its brush-to-device transform, then composed source-over.

use alloc::vec::Vec;

use crate::{Matrix, Point, RectF, pt, push};

/// Samples per pixel side when antialiasing.
const GRID: usize = 4;

/// A flattened figure: its points, whether it is closed and whether
/// fills include it (D2D1_FIGURE_BEGIN_FILLED).
pub struct Poly {
    pub pts: Vec<Point>,
    pub closed: bool,
    pub filled: bool,
}

/// Device-space polygons ready to fill (each implicitly closed).
pub type Polys = Vec<Vec<Point>>;

/// How far (in device pixels) a flattened curve may stray.
const TOLERANCE: f32 = 0.1;

/// Appends the cubic Bézier p0..p3 (p0 already in `out`).
pub fn flatten_cubic(out: &mut Vec<Point>, p0: Point, p1: Point, p2: Point, p3: Point) -> bool {
    let dd = |a: Point, b: Point, c: Point| {
        let x = a.x - 2.0 * b.x + c.x;
        let y = a.y - 2.0 * b.y + c.y;
        libm::sqrtf(x * x + y * y)
    };
    let d = dd(p0, p1, p2).max(dd(p1, p2, p3));
    let n = (libm::sqrtf(d * 0.75 / TOLERANCE) as usize).clamp(1, 512);
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let u = 1.0 - t;
        let (a, b, c, e) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        let q = pt(
            a * p0.x + b * p1.x + c * p2.x + e * p3.x,
            a * p0.y + b * p1.y + c * p2.y + e * p3.y,
        );
        if !push(out, q) {
            return false;
        }
    }
    true
}

/// The polygon's signed area (positive: clockwise on screen, y down).
fn signed_area(p: &[Point]) -> f32 {
    let mut a = 0.0;
    for i in 0..p.len() {
        let (u, v) = (p[i], p[(i + 1) % p.len()]);
        a += u.x * v.y - v.x * u.y;
    }
    a / 2.0
}

/// Adds polygon `p` to `out` turned clockwise (so the winding rule unites
/// the pieces of a stroke).
fn add_piece(out: &mut Polys, mut p: Vec<Point>) -> bool {
    if signed_area(&p) < 0.0 {
        p.reverse();
    }
    push(out, p)
}

fn piece<const N: usize>(pts: [Point; N]) -> Option<Vec<Point>> {
    let mut v = Vec::new();
    v.try_reserve_exact(N).ok()?;
    v.extend_from_slice(&pts);
    Some(v)
}

/// D2D1_CAP_STYLE, D2D1_LINE_JOIN and the miter limit of a stroke.
#[derive(Clone, Copy)]
pub struct StrokeStyle {
    pub start_cap: u32,
    pub end_cap: u32,
    pub join: u32,
    pub miter_limit: f32,
}

pub const CAP_FLAT: u32 = 0;
pub const CAP_SQUARE: u32 = 1;
pub const CAP_ROUND: u32 = 2;
pub const CAP_TRIANGLE: u32 = 3;
pub const JOIN_MITER: u32 = 0;
pub const JOIN_BEVEL: u32 = 1;
pub const JOIN_ROUND: u32 = 2;

impl StrokeStyle {
    pub const DEFAULT: StrokeStyle =
        StrokeStyle { start_cap: CAP_FLAT, end_cap: CAP_FLAT, join: JOIN_MITER, miter_limit: 10.0 };
}

fn sub(a: Point, b: Point) -> Point {
    pt(a.x - b.x, a.y - b.y)
}

fn add(a: Point, b: Point) -> Point {
    pt(a.x + b.x, a.y + b.y)
}

fn mul(a: Point, k: f32) -> Point {
    pt(a.x * k, a.y * k)
}

fn len(a: Point) -> f32 {
    libm::sqrtf(a.x * a.x + a.y * a.y)
}

/// A circle of radius `r` about `c` as a polygon.
fn circle(c: Point, r: f32) -> Option<Vec<Point>> {
    let n = ((r * 4.0) as usize).clamp(8, 128);
    let mut v = Vec::new();
    v.try_reserve_exact(n).ok()?;
    for i in 0..n {
        let a = i as f32 * (2.0 * core::f32::consts::PI / n as f32);
        v.push(pt(c.x + r * libm::cosf(a), c.y + r * libm::sinf(a)));
    }
    Some(v)
}

/// The polygons of a stroke `width` wide along each figure of `figs`
/// (already in device space; `width` in device pixels).
pub fn stroke(figs: &[Poly], width: f32, style: &StrokeStyle) -> Option<Polys> {
    let h = width / 2.0;
    let mut out = Vec::new();
    for f in figs {
        // Drop repeated points: they have no direction.
        let mut p: Vec<Point> = Vec::new();
        for &q in &f.pts {
            if p.last().is_none_or(|l| len(sub(q, *l)) > 1e-6) && !push(&mut p, q) {
                return None;
            }
        }
        if f.closed && p.len() > 1 && len(sub(p[0], p[p.len() - 1])) <= 1e-6 {
            p.pop();
        }
        if p.len() == 1 {
            // A zero-length segment: only round and square caps show.
            if style.start_cap == CAP_ROUND && !add_piece(&mut out, circle(p[0], h)?) {
                return None;
            }
            if style.start_cap == CAP_SQUARE {
                let c = p[0];
                let sq = piece([
                    pt(c.x - h, c.y - h),
                    pt(c.x + h, c.y - h),
                    pt(c.x + h, c.y + h),
                    pt(c.x - h, c.y + h),
                ])?;
                if !add_piece(&mut out, sq) {
                    return None;
                }
            }
            continue;
        }
        if p.len() < 2 {
            continue;
        }
        let n = p.len();
        let segs = if f.closed { n } else { n - 1 };
        let normal = |i: usize| {
            let d = sub(p[(i + 1) % n], p[i]);
            let l = len(d);
            pt(-d.y / l * h, d.x / l * h)
        };
        for i in 0..segs {
            let (a, b) = (p[i], p[(i + 1) % n]);
            let nv = normal(i);
            let mut a2 = a;
            let mut b2 = b;
            let dir = mul(sub(b, a), 1.0 / len(sub(b, a)));
            if !f.closed && i == 0 && style.start_cap == CAP_SQUARE {
                a2 = sub(a, mul(dir, h));
            }
            if !f.closed && i == segs - 1 && style.end_cap == CAP_SQUARE {
                b2 = add(b, mul(dir, h));
            }
            if !add_piece(&mut out, piece([add(a2, nv), add(b2, nv), sub(b2, nv), sub(a2, nv)])?) {
                return None;
            }
        }
        // Joins at the inner vertices (every vertex of a closed figure).
        let joins = if f.closed { 0..n } else { 1..n - 1 };
        for i in joins {
            let prev = (i + n - 1) % n;
            let (n0, n1) = (normal(prev), normal(i));
            let c = p[i];
            let cross = n0.x * n1.y - n0.y * n1.x;
            if cross.abs() < 1e-9 && n0.x * n1.x + n0.y * n1.y > 0.0 {
                continue; // straight on
            }
            // The outer side is where the normals spread apart.
            let (o0, o1) =
                if cross > 0.0 { (sub(c, n0), sub(c, n1)) } else { (add(c, n0), add(c, n1)) };
            let ok = match style.join {
                JOIN_ROUND => add_piece(&mut out, circle(c, h)?),
                JOIN_BEVEL => add_piece(&mut out, piece([c, o0, o1])?),
                _ => {
                    // The miter's tip: where the two outer edges meet.
                    let m = mul(add(sub(o0, c), sub(o1, c)), 0.5);
                    let ml = len(m);
                    let ratio = if ml > 1e-9 { h * h / (ml * ml) } else { f32::INFINITY };
                    let tip = add(c, mul(m, ratio));
                    if ratio * ml / h <= style.miter_limit.max(1.0) {
                        add_piece(&mut out, piece([c, o0, tip, o1])?)
                    } else if style.join == JOIN_MITER {
                        // Clipped at the limit (MITER), not beveled.
                        let k = style.miter_limit.max(1.0) * h / ml.max(1e-9);
                        add_piece(&mut out, piece([c, o0, add(c, mul(m, k)), o1])?)
                    } else {
                        add_piece(&mut out, piece([c, o0, o1])?)
                    }
                }
            };
            if !ok {
                return None;
            }
        }
        if !f.closed {
            for (cap, end, other) in
                [(style.start_cap, p[0], p[1]), (style.end_cap, p[n - 1], p[n - 2])]
            {
                let ok = match cap {
                    CAP_ROUND => add_piece(&mut out, circle(end, h)?),
                    CAP_TRIANGLE => {
                        let d = sub(end, other);
                        let u = mul(d, 1.0 / len(d));
                        let nv = pt(-u.y * h, u.x * h);
                        add_piece(
                            &mut out,
                            piece([add(end, nv), add(end, mul(u, h)), sub(end, nv)])?,
                        )
                    }
                    _ => true,
                };
                if !ok {
                    return None;
                }
            }
        }
    }
    Some(out)
}

/// A device-space rectangle of pixels [x0, x1) x [y0, y1).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct IRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl IRect {
    pub fn empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    pub fn intersect(&self, o: &IRect) -> IRect {
        IRect {
            x0: self.x0.max(o.x0),
            y0: self.y0.max(o.y0),
            x1: self.x1.min(o.x1),
            y1: self.y1.min(o.y1),
        }
    }
}

/// The clip: a pixel rectangle and, for antialiased clips, the exact
/// rectangle whose overlap with each pixel scales its coverage.
#[derive(Clone, Copy)]
pub struct Clip {
    pub pixels: IRect,
    pub exact: Option<RectF>,
}

impl Clip {
    /// The share of pixel (x, y) inside the clip.
    fn share(&self, x: i32, y: i32) -> f32 {
        match self.exact {
            None => 1.0,
            Some(r) => {
                let w = (r.right.min(x as f32 + 1.0) - r.left.max(x as f32)).clamp(0.0, 1.0);
                let h = (r.bottom.min(y as f32 + 1.0) - r.top.max(y as f32)).clamp(0.0, 1.0);
                w * h
            }
        }
    }
}

/// The coverage of each pixel of `area` (0 to 1, row by row) by the
/// polygons under the fill rule.
pub struct Mask {
    pub area: IRect,
    pub cov: Vec<f32>,
}

/// Coverage of `polys` within `clip` (None when out of memory; an empty
/// mask when nothing is inside).
pub fn coverage(polys: &[Vec<Point>], winding: bool, antialias: bool, clip: &Clip) -> Option<Mask> {
    // The polygons' bounds, then the area both they and the clip cover.
    let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in polys {
        for q in p {
            if !q.x.is_finite() || !q.y.is_finite() {
                return Some(Mask { area: IRect { x0: 0, y0: 0, x1: 0, y1: 0 }, cov: Vec::new() });
            }
            minx = minx.min(q.x);
            miny = miny.min(q.y);
            maxx = maxx.max(q.x);
            maxy = maxy.max(q.y);
        }
    }
    let bounds = IRect {
        x0: libm::floorf(minx.max(-1.0e7)) as i32,
        y0: libm::floorf(miny.max(-1.0e7)) as i32,
        x1: libm::ceilf(maxx.min(1.0e7)) as i32 + 1,
        y1: libm::ceilf(maxy.min(1.0e7)) as i32 + 1,
    };
    let area = bounds.intersect(&clip.pixels);
    if area.empty() || polys.is_empty() {
        return Some(Mask { area: IRect { x0: 0, y0: 0, x1: 0, y1: 0 }, cov: Vec::new() });
    }
    let w = (area.x1 - area.x0) as usize;
    let h = (area.y1 - area.y0) as usize;
    let mut cov = crate::filled(w * h, 0.0f32)?;
    // The edges, each with its direction (+1 downwards).
    let mut edges: Vec<(Point, Point, i32)> = Vec::new();
    for p in polys {
        for i in 0..p.len() {
            let (a, b) = (p[i], p[(i + 1) % p.len()]);
            if a.y == b.y {
                continue;
            }
            let e = if a.y < b.y { (a, b, 1) } else { (b, a, -1) };
            if !push(&mut edges, e) {
                return None;
            }
        }
    }
    edges.sort_unstable_by(|a, b| a.0.y.total_cmp(&b.0.y));
    let s = if antialias { GRID } else { 1 };
    let cols = w * s;
    let mut diff = crate::filled(cols + 1, 0i32)?;
    let mut xs: Vec<(f32, i32)> = Vec::new();
    for row in 0..h {
        let y = (area.y0 + row as i32) as f32;
        diff.iter_mut().for_each(|d| *d = 0);
        let mut any = false;
        for k in 0..s {
            let sy = y + (k as f32 + 0.5) / s as f32;
            xs.clear();
            for e in &edges {
                if e.0.y > sy {
                    break;
                }
                if sy < e.1.y {
                    let x = e.0.x + (sy - e.0.y) * (e.1.x - e.0.x) / (e.1.y - e.0.y);
                    if !push(&mut xs, (x, e.2)) {
                        return None;
                    }
                }
            }
            if xs.is_empty() {
                continue;
            }
            xs.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
            let mut wind = 0;
            for i in 0..xs.len() {
                let was = if winding { wind != 0 } else { wind & 1 != 0 };
                wind += xs[i].1;
                let now = if winding { wind != 0 } else { wind & 1 != 0 };
                if !was && now {
                    // Find where this span ends.
                    let start = xs[i].0;
                    let mut wnd = wind;
                    let mut end = f32::MAX;
                    for x in &xs[i + 1..] {
                        wnd += x.1;
                        let inside = if winding { wnd != 0 } else { wnd & 1 != 0 };
                        if !inside {
                            end = x.0;
                            break;
                        }
                    }
                    // Samples at (c + 0.5) / s from the area's left edge.
                    let to_col = |x: f32| {
                        let v = libm::ceilf((x - area.x0 as f32) * s as f32 - 0.5);
                        v.clamp(0.0, cols as f32) as usize
                    };
                    let (c0, c1) = (to_col(start), to_col(end));
                    if c0 < c1 {
                        diff[c0] += 1;
                        diff[c1] -= 1;
                        any = true;
                    }
                }
            }
        }
        if !any {
            continue;
        }
        let mut run = 0;
        let full = (s * s) as f32;
        let line = &mut cov[row * w..(row + 1) * w];
        for (px, c) in line.iter_mut().enumerate() {
            let mut n = 0;
            for j in 0..s {
                run += diff[px * s + j];
                n += run;
            }
            *c = n as f32 / full;
        }
    }
    if clip.exact.is_some() {
        for row in 0..h {
            for col in 0..w {
                let share = clip.share(area.x0 + col as i32, area.y0 + row as i32);
                cov[row * w + col] *= share;
            }
        }
    }
    Some(Mask { area, cov })
}

/// A premultiplied colour, components 0 to 1.
#[derive(Clone, Copy, Default, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub fn premultiplied(c: crate::ColorF, opacity: f32) -> Rgba {
        let a = (c.a * opacity).clamp(0.0, 1.0);
        Rgba {
            r: c.r.clamp(0.0, 1.0) * a,
            g: c.g.clamp(0.0, 1.0) * a,
            b: c.b.clamp(0.0, 1.0) * a,
            a,
        }
    }

    fn lerp(self, o: Rgba, t: f32) -> Rgba {
        Rgba {
            r: self.r + (o.r - self.r) * t,
            g: self.g + (o.g - self.g) * t,
            b: self.b + (o.b - self.b) * t,
            a: self.a + (o.a - self.a) * t,
        }
    }

    fn scale(self, k: f32) -> Rgba {
        Rgba { r: self.r * k, g: self.g * k, b: self.b * k, a: self.a * k }
    }
}

/// A gradient's stops, sorted by position, premultiplied.
pub struct Ramp {
    pub stops: Vec<(f32, Rgba)>,
    pub extend: u32,
}

pub const EXTEND_CLAMP: u32 = 0;
pub const EXTEND_WRAP: u32 = 1;
pub const EXTEND_MIRROR: u32 = 2;

/// `t` folded into [0, 1] by the extend mode (clamp leaves it as it is).
fn extend(t: f32, mode: u32) -> f32 {
    match mode {
        EXTEND_WRAP => t - libm::floorf(t),
        EXTEND_MIRROR => {
            let u = t - 2.0 * libm::floorf(t / 2.0);
            if u > 1.0 { 2.0 - u } else { u }
        }
        _ => t,
    }
}

impl Ramp {
    fn at(&self, t: f32) -> Rgba {
        let t = extend(t, self.extend);
        let s = &self.stops;
        if s.is_empty() {
            return Rgba::default();
        }
        if t <= s[0].0 {
            return s[0].1;
        }
        for i in 1..s.len() {
            if t <= s[i].0 {
                let (p0, p1) = (s[i - 1].0, s[i].0);
                let k = if p1 > p0 { (t - p0) / (p1 - p0) } else { 1.0 };
                return s[i - 1].1.lerp(s[i].1, k);
            }
        }
        s[s.len() - 1].1
    }
}

/// A bitmap's pixels as a paint reads them (premultiplied BGRA).
pub struct Pixels<'a> {
    pub data: &'a [u8],
    pub w: u32,
    pub h: u32,
    pub stride: usize,
}

impl Pixels<'_> {
    fn get(&self, x: i32, y: i32) -> Rgba {
        let i = y as usize * self.stride + x as usize * 4;
        let b = &self.data[i..i + 4];
        Rgba {
            r: f32::from(b[2]) / 255.0,
            g: f32::from(b[1]) / 255.0,
            b: f32::from(b[0]) / 255.0,
            a: f32::from(b[3]) / 255.0,
        }
    }
}

/// What a draw fills with: sampled at a device point.
pub enum Paint<'a> {
    Solid(Rgba),
    /// Device -> brush space, the axis start and end, the stops, opacity.
    Linear {
        inv: Matrix,
        start: Point,
        end: Point,
        ramp: &'a Ramp,
        opacity: f32,
    },
    /// Device -> brush space, centre, origin offset, radii, stops, opacity.
    Radial {
        inv: Matrix,
        center: Point,
        offset: Point,
        rx: f32,
        ry: f32,
        ramp: &'a Ramp,
        opacity: f32,
    },
    /// Device -> bitmap pixel space, the pixels, extend modes, linear
    /// interpolation, opacity.
    Bitmap {
        inv: Matrix,
        px: Pixels<'a>,
        ex: u32,
        ey: u32,
        linear: bool,
        opacity: f32,
    },
}

/// Pixel index `v` folded into 0..n by the extend mode (clamp repeats
/// the edge pixel).
fn wrap_index(v: i32, n: u32, mode: u32) -> i32 {
    let n = n as i32;
    match mode {
        EXTEND_WRAP => v.rem_euclid(n),
        EXTEND_MIRROR => {
            let m = v.rem_euclid(2 * n);
            if m >= n { 2 * n - 1 - m } else { m }
        }
        _ => v.clamp(0, n - 1),
    }
}

impl Paint<'_> {
    fn at(&self, d: Point) -> Rgba {
        match self {
            Paint::Solid(c) => *c,
            Paint::Linear { inv, start, end, ramp, opacity } => {
                let p = inv.apply(d);
                let v = sub(*end, *start);
                let l2 = v.x * v.x + v.y * v.y;
                let t = if l2 > 0.0 {
                    ((p.x - start.x) * v.x + (p.y - start.y) * v.y) / l2
                } else {
                    0.0
                };
                ramp.at(t).scale(*opacity)
            }
            Paint::Radial { inv, center, offset, rx, ry, ramp, opacity } => {
                if *rx <= 0.0 || *ry <= 0.0 {
                    return Rgba::default();
                }
                let p = inv.apply(d);
                // In the unit circle's space: the origin o, the point q;
                // t = |q - o| / |e - o| where e is the circle's point on
                // the ray from o through q.
                let q = pt((p.x - center.x) / rx, (p.y - center.y) / ry);
                let o = pt(offset.x / rx, offset.y / ry);
                let dv = sub(q, o);
                let dl = len(dv);
                if dl < 1e-9 {
                    return ramp.at(0.0).scale(*opacity);
                }
                let u = mul(dv, 1.0 / dl);
                // |o + s u| = 1: s^2 + 2 (o.u) s + |o|^2 - 1 = 0.
                let b = o.x * u.x + o.y * u.y;
                let c = o.x * o.x + o.y * o.y - 1.0;
                let disc = b * b - c;
                if disc < 0.0 {
                    return Rgba::default();
                }
                let s = -b + libm::sqrtf(disc);
                if s <= 1e-9 {
                    return Rgba::default();
                }
                ramp.at(dl / s).scale(*opacity)
            }
            Paint::Bitmap { inv, px, ex, ey, linear, opacity } => {
                let p = inv.apply(d);
                if px.w == 0 || px.h == 0 {
                    return Rgba::default();
                }
                let c = if *linear {
                    let fx = p.x - 0.5;
                    let fy = p.y - 0.5;
                    let (x0, y0) = (libm::floorf(fx), libm::floorf(fy));
                    let (tx, ty) = (fx - x0, fy - y0);
                    let (x0, y0) = (x0 as i32, y0 as i32);
                    let g =
                        |x: i32, y: i32| px.get(wrap_index(x, px.w, *ex), wrap_index(y, px.h, *ey));
                    let top = g(x0, y0).lerp(g(x0 + 1, y0), tx);
                    let bottom = g(x0, y0 + 1).lerp(g(x0 + 1, y0 + 1), tx);
                    top.lerp(bottom, ty)
                } else {
                    let x = libm::floorf(p.x) as i32;
                    let y = libm::floorf(p.y) as i32;
                    px.get(wrap_index(x, px.w, *ex), wrap_index(y, px.h, *ey))
                };
                c.scale(*opacity)
            }
        }
    }
}

/// A target's pixels: 32 bpp premultiplied, BGRA (or RGBA when `rgba`),
/// rows `stride` bytes apart.
pub struct Surface {
    pub ptr: *mut u8,
    pub w: u32,
    pub h: u32,
    pub stride: usize,
    pub rgba: bool,
    /// D2D1_ALPHA_MODE_IGNORE: the alpha byte is kept opaque.
    pub opaque: bool,
}

impl Surface {
    pub fn bounds(&self) -> IRect {
        IRect { x0: 0, y0: 0, x1: self.w as i32, y1: self.h as i32 }
    }

    fn pixel(&mut self, x: i32, y: i32) -> &mut [u8] {
        let i = y as usize * self.stride + x as usize * 4;
        // SAFETY: (x, y) is inside the surface, whose rows the owner keeps.
        unsafe { core::slice::from_raw_parts_mut(self.ptr.add(i), 4) }
    }

    fn write(&mut self, x: i32, y: i32, c: Rgba) {
        let (rgba, opaque) = (self.rgba, self.opaque);
        let px = self.pixel(x, y);
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        let (r, b) = if rgba { (0, 2) } else { (2, 0) };
        px[r] = q(c.r);
        px[1] = q(c.g);
        px[b] = q(c.b);
        px[3] = if opaque { 255 } else { q(c.a) };
    }

    fn read(&mut self, x: i32, y: i32) -> Rgba {
        let rgba = self.rgba;
        let px = self.pixel(x, y);
        let (r, b) = if rgba { (0, 2) } else { (2, 0) };
        Rgba {
            r: f32::from(px[r]) / 255.0,
            g: f32::from(px[1]) / 255.0,
            b: f32::from(px[b]) / 255.0,
            a: f32::from(px[3]) / 255.0,
        }
    }

    /// Source-over of `paint` through `mask`.
    pub fn compose(&mut self, mask: &Mask, paint: &Paint) {
        let w = (mask.area.x1 - mask.area.x0) as usize;
        for (i, &c) in mask.cov.iter().enumerate() {
            if c <= 0.0 {
                continue;
            }
            let x = mask.area.x0 + (i % w) as i32;
            let y = mask.area.y0 + (i / w) as i32;
            let s = paint.at(pt(x as f32 + 0.5, y as f32 + 0.5)).scale(c);
            if s.a <= 0.0 && s.r <= 0.0 && s.g <= 0.0 && s.b <= 0.0 {
                continue;
            }
            let d = self.read(x, y);
            let k = 1.0 - s.a;
            self.write(
                x,
                y,
                Rgba { r: s.r + d.r * k, g: s.g + d.g * k, b: s.b + d.b * k, a: s.a + d.a * k },
            );
        }
    }

    /// Clear: the colour replaces what is inside the clip.
    pub fn fill(&mut self, clip: &Clip, c: Rgba) {
        let r = clip.pixels.intersect(&self.bounds());
        for y in r.y0..r.y1 {
            for x in r.x0..r.x1 {
                let share = clip.share(x, y);
                if share >= 1.0 {
                    self.write(x, y, c);
                } else if share > 0.0 {
                    let d = self.read(x, y);
                    self.write(x, y, d.lerp(c, share));
                }
            }
        }
    }
}
