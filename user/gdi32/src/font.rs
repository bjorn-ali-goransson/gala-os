//! Fonts: a LOGFONTW kept as given, realized with one of gala's two
//! families. Proportional faces ("Segoe UI", "MS Shell Dlg", "Tahoma",
//! "Arial", ... and the default) are Selawik, an open font with Segoe
//! UI's metrics, rasterized at any size with fontdue (four weights:
//! semilight, regular, semibold, bold). Fixed-pitch faces ("Courier
//! New", "Consolas", ..., FIXED_PITCH or FF_MODERN) are Noto Sans Mono's
//! bitmaps (16, 20, 24 and 32 pixels; regular and bold) scaled to any
//! height, every character the same width.
//!
//! Metrics follow GDI's: a negative lfHeight is the character (em)
//! height, a positive one the cell height, 0 the default (12-pixel characters). For Selawik
//! the cell is the font's Windows ascent + descent (2210 + 514 units of
//! 2048), each rounded, so a 12-pixel em gives Windows' 16-pixel cell
//! with a 13-pixel ascent; advances are the hinted ones, each rounded to
//! whole pixels (GDI adds them up the same way), no kerning. For Noto
//! the cell is 4/3 of the em, the ascent 13/16 of the cell.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use noto_sans_mono_bitmap::{FontWeight, RasterHeight, get_raster, get_raster_width};

/// LOGFONTW.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LogFont {
    pub height: i32,
    pub width: i32,
    pub escapement: i32,
    pub orientation: i32,
    pub weight: i32,
    pub italic: u8,
    pub underline: u8,
    pub strike_out: u8,
    pub char_set: u8,
    pub out_precision: u8,
    pub clip_precision: u8,
    pub quality: u8,
    pub pitch_and_family: u8,
    pub face: [u16; 32],
}

impl LogFont {
    /// The default GUI font: 12-pixel characters, "Segoe UI".
    pub const fn default_gui() -> LogFont {
        let mut face = [0u16; 32];
        let name = b"Segoe UI";
        let mut i = 0;
        while i < name.len() {
            face[i] = name[i] as u16;
            i += 1;
        }
        LogFont {
            height: -12,
            width: 0,
            escapement: 0,
            orientation: 0,
            weight: 400,
            italic: 0,
            underline: 0,
            strike_out: 0,
            char_set: 1, // DEFAULT_CHARSET
            out_precision: 0,
            clip_precision: 0,
            quality: 0,
            pitch_and_family: 0,
            face,
        }
    }

    /// Whether the font asks for a fixed-pitch face: FIXED_PITCH, or
    /// FF_MODERN without a name, or a fixed-pitch face's name.
    fn fixed(&self) -> bool {
        const FIXED: [&[u8]; 9] = [
            b"courier",
            b"courier new",
            b"consolas",
            b"lucida console",
            b"lucida sans typewriter",
            b"terminal",
            b"fixedsys",
            b"cascadia mono",
            b"cascadia code",
        ];
        let len = self.face.iter().position(|&c| c == 0).unwrap_or(32);
        let name = &self.face[..len];
        if self.pitch_and_family & 3 == 1 || (len == 0 && self.pitch_and_family & 0xf0 == 0x30) {
            return true;
        }
        let lower = |c: u16| if (0x41..=0x5a).contains(&c) { c + 0x20 } else { c };
        FIXED.iter().any(|f| {
            f.len() == name.len() && f.iter().zip(name).all(|(&a, &b)| u16::from(a) == lower(b))
        })
    }
}

/// The Selawik files (OFL; THIRD_PARTY.md) by weight.
static SELAWIK: [&[u8]; 4] = [
    include_bytes!("../../../fonts/selawik/selawksl.ttf"),
    include_bytes!("../../../fonts/selawik/selawk.ttf"),
    include_bytes!("../../../fonts/selawik/selawksb.ttf"),
    include_bytes!("../../../fonts/selawik/selawkb.ttf"),
];

#[derive(Clone, Copy)]
enum Kind {
    Mono {
        raster: RasterHeight,
        bold: bool,
        width: i32,
    },
    /// A Selawik weight and the em in 1/64 pixels.
    Selawik {
        face: usize,
        em64: u32,
    },
}

/// A font realized for drawing and measuring.
#[derive(Clone, Copy)]
pub struct Metrics {
    /// The cell height (tmHeight) and the em (tmHeight - internal leading).
    pub height: i32,
    pub em: i32,
    pub ascent: i32,
    /// The average and the widest advance.
    pub average: i32,
    pub widest: i32,
    pub weight: i32,
    pub italic: bool,
    pub underline: bool,
    pub strike_out: bool,
    /// lfWidth when given (every advance is scaled to it for Noto).
    width: i32,
    kind: Kind,
}

const SIZES: [(i32, RasterHeight); 4] = [
    (16, RasterHeight::Size16),
    (20, RasterHeight::Size20),
    (24, RasterHeight::Size24),
    (32, RasterHeight::Size32),
];

fn round_div(a: i64, b: i64) -> i32 {
    ((2 * a + b) / (2 * b)) as i32
}

pub fn metrics(f: &LogFont) -> Metrics {
    let weight = if f.weight == 0 { 400 } else { f.weight };
    if f.fixed() {
        return mono(f, weight);
    }
    let face = match weight {
        ..=350 => 0,
        351..=550 => 1,
        551..=650 => 2,
        _ => 3,
    };
    let Ok(t) = ttf_parser::Face::parse(SELAWIK[face], 0) else { return mono(f, weight) };
    let upem = i64::from(t.units_per_em());
    let (asc, desc) = match t.tables().os2 {
        Some(os2) => (i64::from(os2.windows_ascender()), i64::from(os2.windows_descender()).abs()),
        None => (i64::from(t.ascender()), i64::from(t.descender()).abs()),
    };
    // The em in 1/64 pixels.
    let em64 = match f.height {
        h if h < 0 => i64::from(-h.max(-1024)) * 64,
        0 => 12 * 64,
        h => i64::from(h.min(1024)) * 64 * upem / (asc + desc),
    }
    .max(64);
    let ascent = round_div(em64 * asc, 64 * upem);
    let height = ascent + round_div(em64 * desc, 64 * upem);
    let average = selawik_average(SELAWIK[face])
        .map_or(em64 as i32 / 128, |avg| round_div(em64 * avg, 64 * upem));
    let kind = Kind::Selawik { face, em64: em64 as u32 };
    let mut m = Metrics {
        height,
        em: round_div(em64, 64).min(height),
        ascent,
        average: average.max(1),
        widest: 0,
        weight,
        italic: f.italic != 0,
        underline: f.underline != 0,
        strike_out: f.strike_out != 0,
        width: 0,
        kind,
    };
    m.widest = (0x20u8..0x7f).map(|c| m.advance(char::from(c))).max().unwrap_or(1);
    m
}

/// OS/2's xAvgCharWidth (the second field), in font units.
fn selawik_average(data: &[u8]) -> Option<i64> {
    let raw = ttf_parser::RawFace::parse(data, 0).ok()?;
    let os2 = raw.table(ttf_parser::Tag::from_bytes(b"OS/2"))?;
    Some(i64::from(i16::from_be_bytes([*os2.get(2)?, *os2.get(3)?])))
}

fn mono(f: &LogFont, weight: i32) -> Metrics {
    let (height, em) = match f.height {
        h if h < 0 => {
            let em = -h;
            ((em * 4 + 2) / 3, em)
        }
        0 => (16, 12),
        h => (h, (h * 3 + 2) / 4),
    };
    let height = height.clamp(1, 1024);
    let bold = weight >= 600;
    let (r, raster) = SIZES.iter().copied().find(|&(r, _)| r >= height).unwrap_or(SIZES[3]);
    let w =
        get_raster_width(if bold { FontWeight::Bold } else { FontWeight::Regular }, raster) as i32;
    let advance = if f.width > 0 { f.width } else { ((w * height + r / 2) / r).max(1) };
    Metrics {
        height,
        em: em.min(height),
        ascent: (height * 13 + 8) / 16,
        average: advance,
        widest: advance,
        weight,
        italic: f.italic != 0,
        underline: f.underline != 0,
        strike_out: f.strike_out != 0,
        width: advance,
        kind: Kind::Mono { raster, bold, width: w },
    }
}

impl Metrics {
    /// Whether every character advances by the same width.
    pub fn fixed(&self) -> bool {
        matches!(self.kind, Kind::Mono { .. })
    }

    /// Character `c`'s advance in whole pixels.
    pub fn advance(&self, c: char) -> i32 {
        match self.kind {
            Kind::Mono { .. } => self.width,
            Kind::Selawik { face, em64 } => {
                let Ok(t) = ttf_parser::Face::parse(SELAWIK[face], 0) else { return 0 };
                let g = t.glyph_index(c).or_else(|| t.glyph_index('?')).unwrap_or_default();
                let adv = i64::from(t.glyph_hor_advance(g).unwrap_or(0));
                round_div(i64::from(em64) * adv, 64 * i64::from(t.units_per_em()))
            }
        }
    }

    /// The text's width: the advances added up.
    pub fn width(&self, text: &[char]) -> i32 {
        match self.kind {
            Kind::Mono { .. } => self.width * text.len() as i32,
            Kind::Selawik { .. } => text.iter().map(|&c| self.advance(c)).sum(),
        }
    }

    /// Calls `put(x, y, coverage)` for glyph `c`'s pixels, relative to
    /// the top left of its cell (x may be negative or past the advance).
    pub fn glyph(&self, c: char, mut put: impl FnMut(i32, i32, u8)) {
        // Italic: a slant of a quarter of the height above the baseline,
        // leaning right.
        let slant = |y: i32| if self.italic { (self.ascent - y) / 4 } else { 0 };
        match self.kind {
            Kind::Mono { raster, bold, width } => {
                let weight = if bold { FontWeight::Bold } else { FontWeight::Regular };
                let Some(g) =
                    get_raster(c, weight, raster).or_else(|| get_raster('?', weight, raster))
                else {
                    return;
                };
                let rows = g.raster();
                let r = raster.val() as i32;
                for y in 0..self.height {
                    for x in 0..self.width {
                        // Four samples per pixel from the nearest bitmap.
                        let mut sum = 0u32;
                        for i in 0..2 {
                            for j in 0..2 {
                                let sx = ((4 * x + 2 * i + 1) * width) / (4 * self.width);
                                let sy = ((4 * y + 2 * j + 1) * r) / (4 * self.height);
                                if sx >= 0 && sy >= 0 && sx < width && sy < r {
                                    sum += u32::from(rows[sy as usize][sx as usize]);
                                }
                            }
                        }
                        if sum != 0 {
                            put(x + slant(y), y, (sum / 4) as u8);
                        }
                    }
                }
            }
            Kind::Selawik { face, em64 } => with_glyph(face, em64, c, |g| {
                let top = self.ascent - g.ymin - g.h;
                for y in 0..g.h {
                    for x in 0..g.w {
                        let a = g.coverage[(y * g.w + x) as usize];
                        if a != 0 {
                            put(g.xmin + x + slant(top + y), top + y, a);
                        }
                    }
                }
            }),
        }
    }
}

/// A rasterized glyph: its box (x from the pen, y of the bottom row
/// above the baseline) and coverage rows, top first.
struct Glyph {
    face: u8,
    em64: u32,
    c: char,
    xmin: i32,
    ymin: i32,
    w: i32,
    h: i32,
    coverage: Vec<u8>,
}

const BUCKETS: usize = 256;
/// Glyphs kept per bucket; a full bucket starts over.
const PER_BUCKET: usize = 16;

struct Cache {
    lock: AtomicBool,
    fonts: core::cell::UnsafeCell<[Option<fontdue::Font>; 4]>,
    glyphs: core::cell::UnsafeCell<[Vec<Glyph>; BUCKETS]>,
}

// SAFETY: the cells are only touched under `lock`.
unsafe impl Sync for Cache {}

static CACHE: Cache = Cache {
    lock: AtomicBool::new(false),
    fonts: core::cell::UnsafeCell::new([None, None, None, None]),
    glyphs: core::cell::UnsafeCell::new([const { Vec::new() }; BUCKETS]),
};

/// Runs `f` with glyph `c` of a Selawik weight at an em of `em64` / 64
/// pixels, rasterized once and kept.
fn with_glyph(face: usize, em64: u32, c: char, f: impl FnOnce(&Glyph)) {
    while CACHE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock; nothing below takes it again.
    let (fonts, glyphs) = unsafe { (&mut *CACHE.fonts.get(), &mut *CACHE.glyphs.get()) };
    let bucket = &mut glyphs[(c as usize ^ ((em64 as usize >> 4) * 31) ^ (face * 7)) % BUCKETS];
    let found = bucket.iter().position(|g| g.face == face as u8 && g.em64 == em64 && g.c == c);
    let index = match found {
        Some(i) => Some(i),
        None => {
            if fonts[face].is_none() {
                fonts[face] =
                    fontdue::Font::from_bytes(SELAWIK[face], fontdue::FontSettings::default()).ok();
            }
            fonts[face].as_ref().map(|font| {
                let index = font.lookup_glyph_index(c);
                let index = if index == 0 { font.lookup_glyph_index('?') } else { index };
                let (m, coverage) = font.rasterize_indexed(index, em64 as f32 / 64.0);
                if bucket.len() >= PER_BUCKET {
                    bucket.clear();
                }
                bucket.push(Glyph {
                    face: face as u8,
                    em64,
                    c,
                    xmin: m.xmin,
                    ymin: m.ymin,
                    w: m.width as i32,
                    h: m.height as i32,
                    coverage,
                });
                bucket.len() - 1
            })
        }
    };
    if let Some(i) = index {
        f(&bucket[i]);
    }
    CACHE.lock.store(false, Ordering::Release);
}

/// TEXTMETRICW.
#[repr(C)]
pub struct TextMetricW {
    pub height: i32,
    pub ascent: i32,
    pub descent: i32,
    pub internal_leading: i32,
    pub external_leading: i32,
    pub ave_char_width: i32,
    pub max_char_width: i32,
    pub weight: i32,
    pub overhang: i32,
    pub digitized_aspect_x: i32,
    pub digitized_aspect_y: i32,
    pub first_char: u16,
    pub last_char: u16,
    pub default_char: u16,
    pub break_char: u16,
    pub italic: u8,
    pub underlined: u8,
    pub struck_out: u8,
    pub pitch_and_family: u8,
    pub char_set: u8,
}

pub fn text_metrics(f: &LogFont) -> TextMetricW {
    let m = metrics(f);
    TextMetricW {
        height: m.height,
        ascent: m.ascent,
        descent: m.height - m.ascent,
        internal_leading: m.height - m.em,
        external_leading: 0,
        ave_char_width: m.average,
        max_char_width: m.widest,
        weight: m.weight,
        overhang: 0,
        digitized_aspect_x: 96,
        digitized_aspect_y: 96,
        first_char: 0x20,
        last_char: 0xfffc,
        default_char: u16::from(b'?'),
        break_char: u16::from(b' '),
        italic: u8::from(m.italic),
        underlined: u8::from(m.underline),
        struck_out: u8::from(m.strike_out),
        // Noto: fixed pitch (TMPF_FIXED_PITCH clear), FF_MODERN. Selawik:
        // variable pitch, vector, TrueType (TMPF_FIXED_PITCH | TMPF_VECTOR
        // | TMPF_TRUETYPE), FF_SWISS.
        pitch_and_family: if m.fixed() { 0x30 } else { 0x27 },
        char_set: f.char_set,
    }
}
