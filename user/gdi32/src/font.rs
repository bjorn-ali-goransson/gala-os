//! Fonts: a LOGFONTW kept as given, realized with the one face gala has,
//! Noto Sans Mono's bitmaps (16, 20, 24 and 32 pixels; regular and bold),
//! scaled to any height. Metrics follow GDI's: lfHeight < 0 is the
//! character (em) height, > 0 the cell height, 0 the default (12-pixel
//! characters); the cell is 4/3 of the em (a 12-pixel em gets the 16-pixel
//! bitmaps unscaled); tmInternalLeading is the difference; ascent is 13/16
//! of the cell. Every character advances by the same width (a fixed-pitch
//! font, which wx and GDI measure consistently).

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
}

/// A font realized for drawing and measuring.
#[derive(Clone, Copy)]
pub struct Metrics {
    /// The cell height (tmHeight) and the em (tmHeight - internal leading).
    pub height: i32,
    pub em: i32,
    pub ascent: i32,
    /// The advance of every character.
    pub advance: i32,
    pub weight: i32,
    pub italic: bool,
    pub underline: bool,
    pub strike_out: bool,
    raster: RasterHeight,
    bold: bool,
}

const SIZES: [(i32, RasterHeight); 4] = [
    (16, RasterHeight::Size16),
    (20, RasterHeight::Size20),
    (24, RasterHeight::Size24),
    (32, RasterHeight::Size32),
];

pub fn metrics(f: &LogFont) -> Metrics {
    let (height, em) = match f.height {
        h if h < 0 => {
            let em = -h;
            ((em * 4 + 2) / 3, em)
        }
        0 => (16, 12),
        h => (h, (h * 3 + 2) / 4),
    };
    let height = height.clamp(1, 1024);
    let weight = if f.weight == 0 { 400 } else { f.weight };
    let bold = weight >= 600;
    let (r, raster) = SIZES.iter().copied().find(|&(r, _)| r >= height).unwrap_or(SIZES[3]);
    let w =
        get_raster_width(if bold { FontWeight::Bold } else { FontWeight::Regular }, raster) as i32;
    let advance = if f.width > 0 { f.width } else { ((w * height + r / 2) / r).max(1) };
    Metrics {
        height,
        em: em.min(height),
        ascent: (height * 13 + 8) / 16,
        advance,
        weight,
        italic: f.italic != 0,
        underline: f.underline != 0,
        strike_out: f.strike_out != 0,
        raster,
        bold,
    }
}

impl Metrics {
    /// Glyph `c`'s coverage (0-255) at (x, y) of its advance x height
    /// cell, scaled from the nearest bitmap (four samples per pixel).
    pub fn coverage(&self, c: char, x: i32, y: i32) -> u8 {
        let weight = if self.bold { FontWeight::Bold } else { FontWeight::Regular };
        let Some(g) =
            get_raster(c, weight, self.raster).or_else(|| get_raster('?', weight, self.raster))
        else {
            return 0;
        };
        let rows = g.raster();
        let r = self.raster.val() as i32;
        let rw = get_raster_width(weight, self.raster) as i32;
        // Italic: a slant of a quarter of the height, leaning right.
        let x = if self.italic { x - (self.height - y) / 4 } else { x };
        let mut sum = 0u32;
        for i in 0..2 {
            for j in 0..2 {
                let sx = ((4 * x + 2 * i + 1) * rw) / (4 * self.advance);
                let sy = ((4 * y + 2 * j + 1) * r) / (4 * self.height);
                if sx >= 0 && sy >= 0 && sx < rw && sy < r {
                    sum += u32::from(rows[sy as usize][sx as usize]);
                }
            }
        }
        (sum / 4) as u8
    }
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
        ave_char_width: m.advance,
        max_char_width: m.advance,
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
        // Fixed pitch (TMPF_FIXED_PITCH clear), FF_MODERN.
        pitch_and_family: 0x30,
        char_set: f.char_set,
    }
}
