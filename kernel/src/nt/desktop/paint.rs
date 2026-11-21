//! Drawing for the desktop's own parts, the window frames: colours, a
//! small ARGB canvas (outlines, text) for the captions, rounded corners
//! and the soft shadows. Integer arithmetic only (the kernel has no
//! floating point). Colours are 0x00RRGGBB on the screen and 0xAARRGGBB
//! in layers. The shell (taskbar, start menu, wallpaper) is shasha's,
//! drawn in user mode.

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

/// The desktop's font: Selawik (an open-source stand-in for Segoe UI)
/// rasterized at build time (`cargo xtask bake-fonts`), 12 pixels
/// regular, as Windows 11 draws captions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Font {
    Ui12,
}

static UI12: &[u8] = include_bytes!("../../../assets/ui-12-regular.glyphs");

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

    pub fn get(&self, x: i64, y: i64) -> u32 {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return 0;
        }
        self.px.get((y * self.w + x) as usize).copied().unwrap_or(0)
    }
}
