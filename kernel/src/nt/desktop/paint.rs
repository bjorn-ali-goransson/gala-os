//! Drawing for the desktop: colours, a small ARGB canvas (rounded
//! rectangles, gradients, text, pictures), the wallpaper and the soft
//! shadows. Integer arithmetic only (the kernel has no floating point).
//! Colours are 0x00RRGGBB on the screen and 0xAARRGGBB in layers.

use crate::mm::heap::TryVec;
use alloc::vec::Vec;

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

/// The desktop's fonts: Selawik (an open-source stand-in for Segoe UI)
/// rasterized at build time (`cargo xtask bake-fonts`): sizes in pixels,
/// as Windows 11's shell uses them (12 for captions and the taskbar, 14
/// semibold for section headings...).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(dead_code)] // the larger sizes: for the shell's coming parts
pub enum Font {
    Ui12,
    Ui12Semibold,
    Ui14Semibold,
    Ui20Semibold,
    Ui28Semilight,
}

static UI12: &[u8] = include_bytes!("../../../assets/ui-12-regular.glyphs");
static UI12SB: &[u8] = include_bytes!("../../../assets/ui-12-semibold.glyphs");
static UI14SB: &[u8] = include_bytes!("../../../assets/ui-14-semibold.glyphs");
static UI20SB: &[u8] = include_bytes!("../../../assets/ui-20-semibold.glyphs");
static UI28SL: &[u8] = include_bytes!("../../../assets/ui-28-semilight.glyphs");

/// A glyph: advance (1/16 pixel), the bitmap's top-left from the pen at
/// the baseline, its size, its coverage.
struct Glyph<'a> {
    advance: i64,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    coverage: &'a [u8],
}

const TABLE: usize = 10;
const ENTRY: usize = 14;

impl Font {
    fn data(self) -> &'static [u8] {
        match self {
            Font::Ui12 => UI12,
            Font::Ui12Semibold => UI12SB,
            Font::Ui14Semibold => UI14SB,
            Font::Ui20Semibold => UI20SB,
            Font::Ui28Semilight => UI28SL,
        }
    }

    fn i16_at(self, at: usize) -> i64 {
        let d = self.data();
        d.get(at..at + 2).map_or(0, |b| i64::from(i16::from_le_bytes([b[0], b[1]])))
    }

    pub fn ascent(self) -> i64 {
        self.i16_at(6)
    }

    /// The line height (ascent + descent).
    pub fn height(self) -> i64 {
        self.ascent() + self.i16_at(8)
    }

    fn glyph(self, c: char) -> Glyph<'static> {
        let code = u32::from(c);
        let code = if (0x20..=0xff).contains(&code) { code } else { u32::from('?') };
        let d = self.data();
        let e = TABLE + (code as usize - 0x20) * ENTRY;
        let u16_at = |at: usize| {
            d.get(at..at + 2).map_or(0, |b| i64::from(u16::from_le_bytes([b[0], b[1]])))
        };
        let w = u16_at(e + 6);
        let h = u16_at(e + 8);
        let off = d
            .get(e + 10..e + 14)
            .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
        let start = TABLE + 224 * ENTRY + off;
        Glyph {
            advance: self.i16_at(e),
            x: self.i16_at(e + 2),
            y: self.i16_at(e + 4),
            w,
            h,
            coverage: d.get(start..start + (w * h) as usize).unwrap_or(&[]),
        }
    }

    /// The width of `text` in pixels.
    pub fn measure(self, text: impl Iterator<Item = char>) -> i64 {
        (text.map(|c| self.glyph(c).advance).sum::<i64>() + 8) / 16
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

    /// Text with its top at (x, y), no further right than `max_x` (what
    /// does not fit ends with "..."); where it ended.
    pub fn text(
        &mut self,
        x: i64,
        y: i64,
        text: impl Iterator<Item = char> + Clone,
        rgb: u32,
        font: Font,
        max_x: i64,
    ) -> i64 {
        let fits = x + font.measure(text.clone()) <= max_x;
        let dots = font.measure("...".chars());
        let mut pen = x * 16;
        let baseline = y + font.ascent();
        let mut draw = |c: char, pen: &mut i64| {
            let g = font.glyph(c);
            let gx = (*pen + 8) / 16 + g.x;
            for j in 0..g.h {
                for i in 0..g.w {
                    let a = u32::from(g.coverage[(j * g.w + i) as usize]);
                    if a != 0 {
                        self.put(gx + i, baseline + g.y + j, rgb, a);
                    }
                }
            }
            *pen += g.advance;
        };
        for c in text {
            if !fits && (pen + font.glyph(c).advance + 8) / 16 + dots > max_x {
                for d in "...".chars() {
                    draw(d, &mut pen);
                }
                return (pen + 8) / 16;
            }
            draw(c, &mut pen);
        }
        (pen + 8) / 16
    }

    /// A `size` x `size` RGBA picture at (x, y).
    #[allow(dead_code)]
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

    /// A `size` x `size` alpha mask at (x, y) in `top`..`bottom` (a
    /// vertical gradient) at most alpha `a`.
    #[allow(clippy::too_many_arguments)]
    pub fn mask(&mut self, x: i64, y: i64, alpha: &[u8], size: i64, top: u32, bottom: u32, a: u32) {
        for j in 0..size {
            let c = lerp(top, bottom, j, size - 1);
            for i in 0..size {
                let m = u32::from(alpha.get((j * size + i) as usize).copied().unwrap_or(0));
                if m != 0 {
                    self.put(x + i, y + j, c, m * a / 255);
                }
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

/// The integer square root.
fn isqrt(v: i64) -> i64 {
    if v <= 0 {
        return 0;
    }
    let mut x = v;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x
}

/// The wallpaper, in the spirit of Windows 11's (its own design, not
/// Microsoft's artwork): a deep blue lit from just below the middle, with
/// three translucent ribbons of light flowing across. Laid out once per
/// screen size.
pub struct Wallpaper {
    w: i64,
    h: i64,
    /// Each ribbon's centre line per column.
    ribbons: [Vec<i64>; 3],
    /// The watermark (Gala's fortress and name, faint white) and its place.
    mark: Option<(Canvas, i64, i64)>,
    /// A picture scaled to the screen (w x h), when one was chosen; else
    /// the drawn light and ribbons.
    picture: Vec<u32>,
}

/// A decoded picture: its size and 0x00RRGGBB pixels, rows top first.
pub struct Picture {
    pub w: i64,
    pub h: i64,
    pub pixels: Vec<u32>,
}

/// Decodes a QOI image (written from the specification at qoiformat.org:
/// a 14-byte header, then chunks: RGB, RGBA, INDEX into 64 recent
/// colours, DIFF, LUMA, RUN); None when it is malformed, larger than
/// 8192 x 8192, or memory runs out.
pub fn decode_qoi(data: &[u8]) -> Option<Picture> {
    if data.len() < 14 || &data[..4] != b"qoif" {
        return None;
    }
    let w = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let h = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return None;
    }
    let n = w as usize * h as usize;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(n).ok()?;
    let mut index = [[0u8; 4]; 64];
    let mut px = [0u8, 0, 0, 255];
    let mut p = 14;
    while pixels.len() < n {
        let b = *data.get(p)?;
        p += 1;
        let mut run = 1;
        match b {
            0xfe => {
                px[..3].copy_from_slice(data.get(p..p + 3)?);
                p += 3;
            }
            0xff => {
                px.copy_from_slice(data.get(p..p + 4)?);
                p += 4;
            }
            _ => match b >> 6 {
                0 => px = index[usize::from(b & 63)],
                1 => {
                    px[0] = px[0].wrapping_add((b >> 4) & 3).wrapping_sub(2);
                    px[1] = px[1].wrapping_add((b >> 2) & 3).wrapping_sub(2);
                    px[2] = px[2].wrapping_add(b & 3).wrapping_sub(2);
                }
                2 => {
                    let b2 = *data.get(p)?;
                    p += 1;
                    let dg = (b & 0x3f).wrapping_sub(32);
                    px[0] = px[0].wrapping_add(dg).wrapping_sub(8).wrapping_add(b2 >> 4);
                    px[1] = px[1].wrapping_add(dg);
                    px[2] = px[2].wrapping_add(dg).wrapping_sub(8).wrapping_add(b2 & 15);
                }
                _ => run = usize::from(b & 63) + 1,
            },
        }
        let hash = (usize::from(px[0]) * 3
            + usize::from(px[1]) * 5
            + usize::from(px[2]) * 7
            + usize::from(px[3]) * 11)
            % 64;
        index[hash] = px;
        let c = u32::from(px[0]) << 16 | u32::from(px[1]) << 8 | u32::from(px[2]);
        for _ in 0..run.min(n - pixels.len()) {
            pixels.try_push(c).ok()?;
        }
    }
    Some(Picture { w: i64::from(w), h: i64::from(h), pixels })
}

/// Gala's fortress (the project owner's Masmak Fortress drawing, rendered
/// by `cargo xtask bake-icons`) as alpha masks.
pub static FORTRESS24: &[u8; 24 * 24] = include_bytes!("../../../assets/fortress-24.alpha");
pub static FORTRESS96: &[u8; 96 * 96] = include_bytes!("../../../assets/fortress-96.alpha");
/// Gala's wordmark (قلعة, "qal'a"), the owner's lettering: width and
/// height (u16 each), then its alpha mask.
static WORDMARK: &[u8] = include_bytes!("../../../assets/wordmark.alpha");

/// The radial light: from the centre outward.
const LIGHT: [(i64, u32); 4] =
    [(0, 0x00_a9_d1_fb), (380, 0x00_4f_8f_e6), (700, 0x00_1f_55_b8), (1024, 0x00_0b_2a_6e)];
/// The ribbons' colour, thickness (hundredths of the height) and strength.
const RIBBONS: [(u32, i64, i64); 3] =
    [(0x00_dc_ee_ff, 9, 120), (0x00_b6_d8_ff, 6, 95), (0x00_ff_ff_ff, 3, 140)];

impl Wallpaper {
    /// The wallpaper for a w x h screen: `picture` scaled to cover it
    /// (centred, the overflow cropped), or the drawn one.
    pub fn new(w: i64, h: i64, picture: Option<&Picture>) -> Option<Wallpaper> {
        let mut scaled = Vec::new();
        if let Some(pic) = picture.filter(|p| p.w > 0 && p.h > 0 && w > 0 && h > 0) {
            scaled.try_reserve_exact((w * h) as usize).ok()?;
            // Cover: the larger of the two scales (as w / pic.w).
            let (num, den) = if w * pic.h >= h * pic.w { (w, pic.w) } else { (h, pic.h) };
            let (ox, oy) = ((pic.w * num / den - w) / 2, (pic.h * num / den - h) / 2);
            for y in 0..h {
                let sy = ((y + oy) * den / num).clamp(0, pic.h - 1);
                for x in 0..w {
                    let sx = ((x + ox) * den / num).clamp(0, pic.w - 1);
                    let c = pic.pixels.get((sy * pic.w + sx) as usize).copied().unwrap_or(0);
                    scaled.try_push(c).ok()?;
                }
            }
        }
        // (base height, amplitude, degrees across, phase), hundredths.
        let shapes = [(58, 12, 260, 200), (66, 9, 330, 40), (61, 6, 420, 120)];
        let mut ribbons: [Vec<i64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for (k, &(base, amp, f, p)) in shapes.iter().enumerate() {
            let v = &mut ribbons[k];
            v.try_reserve_exact(w as usize).ok()?;
            v.extend(
                (0..w)
                    .map(|x| h * base / 100 - h * amp / 100 * sin1024(x * f / w.max(1) + p) / 1024),
            );
        }
        // The watermark: the fortress over the wordmark, at the bottom
        // right above the taskbar (laid out for it), as faint white.
        let (ww, wh) = (
            i64::from(u16::from_le_bytes([WORDMARK[0], WORDMARK[1]])),
            i64::from(u16::from_le_bytes([WORDMARK[2], WORDMARK[3]])),
        );
        let (mw, mh) = (ww.max(96), 96 + 10 + wh);
        let mut mark = Canvas::new(mw, mh, 0);
        if let Some(m) = mark.as_mut() {
            m.mask((mw - 96) / 2, 0, FORTRESS96, 96, 0x00ff_ffff, 0x00ff_ffff, 255);
            let (x0, y0) = ((mw - ww) / 2, 106);
            for j in 0..wh {
                for i in 0..ww {
                    let a =
                        u32::from(WORDMARK.get(4 + (j * ww + i) as usize).copied().unwrap_or(0));
                    if a != 0 {
                        m.put(x0 + i, y0 + j, 0x00ff_ffff, a);
                    }
                }
            }
        }
        let mark = mark.map(|m| (m, w - mw - 28, h - 48 - mh - 24));
        Some(Wallpaper { w, h, ribbons, mark, picture: scaled })
    }

    /// The drawn wallpaper: a radial blue light and three ribbons.
    fn drawn(&self, x: i64, y: i64) -> u32 {
        let (cx, cy) = (self.w / 2, self.h * 58 / 100);
        let reach = isqrt(cx * cx + cy * cy).max(1);
        let d = isqrt((x - cx) * (x - cx) + (y - cy) * (y - cy)) * 1024 / reach;
        let mut c = gradient(&LIGHT, d.min(1024));
        for (k, &(color, thick, strength)) in RIBBONS.iter().enumerate() {
            let t = (self.h * thick / 100).max(1);
            let off = (y - self.ribbons[k][x as usize]).abs();
            if off < t {
                let f = t - off;
                c = blend(c, color, (strength * f / t * f / t).min(255) as u8);
            }
        }
        c
    }

    pub fn pixel(&self, x: i64, y: i64) -> u32 {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return 0;
        }
        let mut c = match self.picture.get((y * self.w + x) as usize) {
            Some(&c) => c,
            None => self.drawn(x, y),
        };
        if let Some((m, mx, my)) = &self.mark {
            let v = m.get(x - mx, y - my);
            if v >> 24 != 0 {
                // The mark's own alpha, at a fifth: barely there.
                c = blend(c, v & 0x00ff_ffff, ((v >> 24) / 5) as u8);
            }
        }
        c
    }
}
