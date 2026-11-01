//! Drawing for the desktop: colours, a small ARGB canvas (rounded
//! rectangles, gradients, text, pictures), the wallpaper and the soft
//! shadows. Integer arithmetic only (the kernel has no floating point).
//! Colours are 0x00RRGGBB on the screen and 0xAARRGGBB in layers.

use alloc::vec::Vec;
use noto_sans_mono_bitmap::{FontWeight, RasterHeight, get_raster, get_raster_width};

/// `bg` moved toward `fg` by t/255 (alpha bits of the result: none).
pub fn blend(bg: u32, fg: u32, t: u8) -> u32 {
    let t = u32::from(t);
    let mix = |shift: u32| {
        let (a, b) = ((bg >> shift) & 0xff, (fg >> shift) & 0xff);
        ((a * (255 - t) + b * t) / 255) << shift
    };
    mix(16) | mix(8) | mix(0)
}

/// `a` to `b` at i/n.
pub fn lerp(a: u32, b: u32, i: i64, n: i64) -> u32 {
    let t = if n <= 0 { 0 } else { (i.clamp(0, n) * 255 / n) as u8 };
    blend(a, b, t)
}

/// A colour through several stops `(position, colour)` (sorted) at `p`.
pub fn gradient(stops: &[(i64, u32)], p: i64) -> u32 {
    let mut prev = stops[0];
    for &s in stops {
        if p <= s.0 {
            return lerp(prev.1, s.1, p - prev.0, s.0 - prev.0);
        }
        prev = s;
    }
    prev.1
}

/// A layer pixel (straight alpha) over an opaque one.
pub fn over(under: u32, layer: u32) -> u32 {
    blend(under, layer, (layer >> 24) as u8)
}

/// Straight-alpha "over" of `rgb` at alpha `a` onto layer pixel `dst`.
fn over_argb(dst: u32, rgb: u32, a: u32) -> u32 {
    if a == 0 {
        return dst;
    }
    let da = dst >> 24;
    let out_a = a + da * (255 - a) / 255;
    if out_a == 0 {
        return 0;
    }
    let ch = |shift: u32| {
        let s = (rgb >> shift) & 0xff;
        let d = (dst >> shift) & 0xff;
        ((s * a + d * da * (255 - a) / 255) / out_a).min(255) << shift
    };
    out_a << 24 | ch(16) | ch(8) | ch(0)
}

/// How much of pixel (x, y) of a w x h rectangle with corner radius r is
/// inside it (0..=255; 4 x 4 samples in the corners).
pub fn round_coverage(x: i64, y: i64, w: i64, h: i64, r: i64) -> u8 {
    round_coverage2(x, y, w, h, r, r)
}

/// As `round_coverage`, with radius `top` above and `bottom` below.
pub fn round_coverage2(x: i64, y: i64, w: i64, h: i64, top: i64, bottom: i64) -> u8 {
    if x < 0 || y < 0 || x >= w || y >= h {
        return 0;
    }
    let corner = |r: i64, cx: i64, cy: i64| -> u8 {
        // In eighths of a pixel: samples at odd eighths.
        let (cx, cy, r8) = (cx * 8, cy * 8, r * 8);
        let mut n = 0;
        for i in 0..4 {
            for j in 0..4 {
                let (sx, sy) = (x * 8 + 2 * i + 1, y * 8 + 2 * j + 1);
                if (sx - cx) * (sx - cx) + (sy - cy) * (sy - cy) <= r8 * r8 {
                    n += 1;
                }
            }
        }
        (n * 255 / 16) as u8
    };
    if y < top && x < top {
        return corner(top, top, top);
    }
    if y < top && x >= w - top {
        return corner(top, w - top, top);
    }
    if y >= h - bottom && x < bottom {
        return corner(bottom, bottom, h - bottom);
    }
    if y >= h - bottom && x >= w - bottom {
        return corner(bottom, w - bottom, h - bottom);
    }
    255
}

/// The darkness (alpha of black) of a soft shadow `reach` pixels wide
/// around rectangle (x0, y0)-(x1, y1) at point (x, y): `strength` at the
/// edge, falling smoothly to 0.
pub fn shadow_alpha(x: i64, y: i64, r: (i64, i64, i64, i64), reach: i64, strength: i64) -> u8 {
    let dx = (r.0 - x).max(x - r.2 + 1).max(0);
    let dy = (r.1 - y).max(y - r.3 + 1).max(0);
    let d2 = dx * dx + dy * dy;
    let s2 = reach * reach;
    if d2 >= s2 {
        return 0;
    }
    let t = s2 - d2; // s2 at the edge
    (strength * t / s2 * t / s2).clamp(0, 255) as u8
}

/// Fonts: the console's Noto Sans Mono bitmaps.
#[derive(Clone, Copy)]
pub enum Font {
    Regular,
    Bold,
    Large,
}

impl Font {
    fn parts(self) -> (FontWeight, RasterHeight) {
        match self {
            Font::Regular => (FontWeight::Regular, RasterHeight::Size16),
            Font::Bold => (FontWeight::Bold, RasterHeight::Size16),
            Font::Large => (FontWeight::Bold, RasterHeight::Size24),
        }
    }

    pub fn advance(self) -> i64 {
        let (w, h) = self.parts();
        get_raster_width(w, h) as i64
    }

    pub fn height(self) -> i64 {
        self.parts().1.val() as i64
    }
}

/// A w x h ARGB drawing surface.
pub struct Canvas {
    pub w: i64,
    pub h: i64,
    pub px: Vec<u32>,
}

impl Canvas {
    /// Filled with `argb`; None without memory.
    pub fn new(w: i64, h: i64, argb: u32) -> Option<Canvas> {
        let n = usize::try_from(w.max(0) * h.max(0)).ok()?;
        let mut px = Vec::new();
        px.try_reserve_exact(n).ok()?;
        px.resize(n, argb);
        Some(Canvas { w, h, px })
    }

    /// Paints `rgb` at alpha `a` over pixel (x, y).
    pub fn put(&mut self, x: i64, y: i64, rgb: u32, a: u32) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        if let Some(p) = self.px.get_mut((y * self.w + x) as usize) {
            *p = over_argb(*p, rgb & 0x00ff_ffff, a.min(255));
        }
    }

    /// A rounded rectangle of `rgb` at alpha `a`, its colour from `top` to
    /// `bottom` down its height when they differ.
    #[allow(clippy::too_many_arguments)]
    pub fn round_rect(
        &mut self,
        x: i64,
        y: i64,
        w: i64,
        h: i64,
        r: i64,
        top: u32,
        bottom: u32,
        a: u32,
    ) {
        for j in 0..h {
            let c = lerp(top, bottom, j, h - 1);
            for i in 0..w {
                let cov = u32::from(round_coverage(i, j, w, h, r));
                if cov > 0 {
                    self.put(x + i, y + j, c, a * cov / 255);
                }
            }
        }
    }

    /// The outline (1 pixel) of a rounded rectangle.
    #[allow(clippy::too_many_arguments)]
    pub fn round_outline(&mut self, x: i64, y: i64, w: i64, h: i64, r: i64, rgb: u32, a: u32) {
        for j in 0..h {
            for i in 0..w {
                let outer = i64::from(round_coverage(i, j, w, h, r));
                let inner = i64::from(round_coverage(i - 1, j - 1, w - 2, h - 2, (r - 1).max(0)));
                let edge = (outer - inner).max(0) as u32;
                if edge > 0 {
                    self.put(x + i, y + j, rgb, a * edge / 255);
                }
            }
        }
    }

    /// Text at (x, y) (its top), no further right than `max_x`; where it
    /// ended. What the font lacks shows as '?'; text that does not fit
    /// ends with "..".
    pub fn text(
        &mut self,
        x: i64,
        y: i64,
        text: impl Iterator<Item = char> + Clone,
        rgb: u32,
        font: Font,
        max_x: i64,
    ) -> i64 {
        let adv = font.advance();
        let (weight, size) = font.parts();
        let count = text.clone().count() as i64;
        let fits = x + count * adv <= max_x;
        let room = if fits { count } else { ((max_x - x) / adv - 2).max(0) };
        let mut cx = x;
        let dots = if fits { 0 } else { 2 };
        for c in text.take(room as usize).chain(core::iter::repeat_n('.', dots)) {
            let raster = get_raster(c, weight, size).or_else(|| get_raster('?', weight, size));
            if let Some(raster) = raster {
                for (dy, row) in raster.raster().iter().enumerate() {
                    for (dx, &cov) in row.iter().enumerate() {
                        self.put(cx + dx as i64, y + dy as i64, rgb, u32::from(cov));
                    }
                }
            }
            cx += adv;
        }
        cx
    }

    /// A `size` x `size` RGBA picture at (x, y).
    pub fn image(&mut self, x: i64, y: i64, rgba: &[u8], size: i64) {
        for j in 0..size {
            for i in 0..size {
                let k = (4 * (j * size + i)) as usize;
                let Some(p) = rgba.get(k..k + 4) else { return };
                let rgb = u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2]);
                self.put(x + i, y + j, rgb, u32::from(p[3]));
            }
        }
    }

    pub fn get(&self, x: i64, y: i64) -> u32 {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return 0;
        }
        self.px.get((y * self.w + x) as usize).copied().unwrap_or(0)
    }
}

/// sin(degrees) x 1024 (Bhaskara I's approximation; good to ~0.2%).
fn sin1024(deg: i64) -> i64 {
    let d = deg.rem_euclid(360);
    let (d, sign) = if d < 180 { (d, 1) } else { (d - 180, -1) };
    let p = d * (180 - d);
    sign * 4 * p * 1024 / (40500 - p)
}

/// A cheap hash of a point (the stars).
fn hash(x: i64, y: i64) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9e37_79b1) ^ (y as u32).wrapping_mul(0x85eb_ca77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h
}

/// The wallpaper: a desert at dusk. A night sky fading to a warm horizon,
/// stars high up, a low sun with a glow, and three ranges of dunes, the
/// nearer ones darker. Laid out once per screen size.
pub struct Wallpaper {
    w: i64,
    h: i64,
    horizon: i64,
    /// The sky's colour per row.
    sky: Vec<u32>,
    /// Where each dune range starts per column (far to near).
    dunes: [Vec<i64>; 3],
    sun: (i64, i64, i64),
}

/// Dune ranges: (top colour, bottom colour, ridge highlight).
const DUNES: [(u32, u32, u32); 3] = [
    (0x00_d8_92_5a, 0x00_b7_70_44, 0x00_f0_b0_78),
    (0x00_b6_66_3a, 0x00_8c_4a_2a, 0x00_d0_84_50),
    (0x00_86_46_28, 0x00_52_28_18, 0x00_a4_5e_36),
];

impl Wallpaper {
    pub fn new(w: i64, h: i64) -> Option<Wallpaper> {
        let horizon = h * 62 / 100;
        let stops = [
            (0, 0x00_0a_12_2c),
            (h * 28 / 100, 0x00_1c_2a_58),
            (h * 48 / 100, 0x00_5a_48_7c),
            (horizon, 0x00_f4_a2_58),
        ];
        let mut sky = Vec::new();
        sky.try_reserve_exact(h as usize).ok()?;
        sky.extend((0..h).map(|y| gradient(&stops, y)));
        // (base below the horizon, amplitude, two waves: degrees across
        // the screen and phase), all in hundredths of the screen.
        let ranges = [
            (3, 4, (470, 30), (1100, 50)),
            (11, 5, (300, 120), (900, 10)),
            (23, 6, (220, 250), (700, 200)),
        ];
        let mut dunes: [Vec<i64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for (k, &(base, amp, (f1, p1), (f2, p2))) in ranges.iter().enumerate() {
            let v = &mut dunes[k];
            v.try_reserve_exact(w as usize).ok()?;
            let (base, amp) = (horizon + h * base / 100, h * amp / 100);
            v.extend((0..w).map(|x| {
                let s = 2 * sin1024(x * f1 / w.max(1) + p1) + sin1024(x * f2 / w.max(1) + p2);
                base - amp * s / (3 * 1024)
            }));
        }
        let sun = (w * 70 / 100, horizon - h * 3 / 100, (h * 5 / 100).max(4));
        Some(Wallpaper { w, h, horizon, sky, dunes, sun })
    }

    pub fn pixel(&self, x: i64, y: i64) -> u32 {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return 0;
        }
        // The nearest dune range covering the point.
        for k in (0..3).rev() {
            let top = self.dunes[k][x as usize];
            if y >= top {
                let (c0, c1, ridge) = DUNES[k];
                let depth = y - top;
                if depth < 2 {
                    return blend(c0, ridge, if depth == 0 { 200 } else { 110 });
                }
                // Lit from the sun's side: lighter where the slope faces it
                // (the slope over 24 pixels, so the light changes slowly),
                // and the light fades with depth.
                let ahead = self.dunes[k][(x + 12).min(self.w - 1) as usize];
                let behind = self.dunes[k][(x - 12).max(0) as usize];
                let slope = (ahead - behind).clamp(-24, 24);
                let fade = (40 - depth).max(0);
                let light = ((slope + 24) * 70 / 48) * fade / 40 + 12;
                return blend(lerp(c0, c1, depth, self.h - top), 0x00_ff_d0_90, light as u8);
            }
        }
        let mut c = self.sky[y as usize];
        // The sun and its glow.
        let (sx, sy, r) = self.sun;
        let d2 = (x - sx) * (x - sx) + (y - sy) * (y - sy);
        let glow = 9 * r * r;
        if d2 < glow {
            let t = glow - d2;
            c = blend(c, 0x00_ff_c0_78, (150 * t / glow * t / glow) as u8);
        }
        if d2 <= r * r {
            let edge = r * r - d2;
            let a = (edge * 255 / (2 * r).max(1)).min(255) as u8;
            c = blend(c, 0x00_ff_ea_b8, a);
        }
        // Stars, fading toward the horizon.
        let high = self.horizon * 70 / 100;
        if y < high {
            let hsh = hash(x, y);
            if hsh.is_multiple_of(1400) {
                let b = (90 + (hsh >> 20) % 166) as i64 * (high - y) / high;
                c = blend(c, 0x00_ff_ff_ff, b as u8);
            }
        }
        c
    }
}
