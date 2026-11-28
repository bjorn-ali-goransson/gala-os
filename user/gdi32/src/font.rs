//! Fonts: a LOGFONTW kept as given, realized through `FAMILIES` -- the
//! table of families a program finds here, each with the weights and
//! styles it answers for, measured on real Windows (user/gdi32_test's
//! `font_families`). The table decides which names exist, which faces
//! each answers with and which weight a face realizes at; two fonts
//! draw them all. Proportional families are Selawik, an open font with
//! Segoe UI's metrics, rasterized at any size with fontdue (four
//! weights: semilight, regular, semibold, bold). Fixed-pitch families
//! are Noto Sans Mono's bitmaps (16, 20, 24 and 32 pixels; regular and
//! bold) scaled to any height, every character the same width.
//!
//! A name the table has no family for is substituted, as real Windows
//! substitutes one it has no font for, so that **the face a program is
//! given is always a face the system enumerates**: `GetTextFaceW` and
//! the metrics answer for the realized family, and `EnumFontFamiliesExW`
//! walks the same table (`state.rs`). Keeping those two apart is what
//! left a program laying text out in a face it could not then find.
//!
//! Metrics follow GDI's: a negative lfHeight is the character (em)
//! height, a positive one the cell height, 0 the default (12-pixel characters). For Selawik
//! the cell is the font's Windows ascent + descent (2210 + 514 units of
//! 2048), each rounded, so a 12-pixel em gives Windows' 16-pixel cell
//! with a 13-pixel ascent; advances are the hinted ones, each rounded to
//! whole pixels (GDI adds them up the same way), no kerning. For Noto
//! the cell is 4/3 of the em, the ascent 13/16 of the cell.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

    /// The face name as asked for, without its terminator.
    pub fn name(&self) -> &[u16] {
        let len = self.face.iter().position(|&c| c == 0).unwrap_or(32);
        &self.face[..len]
    }
}

/// A font family a program finds here: the name it enumerates and is
/// reported under, the weights of its upright faces (lightest first) and
/// whether it answers italic faces of its own. A program may ask any
/// family for italics, which gala slants; `italic` is what the family
/// *enumerates*.
pub struct Family {
    pub name: &'static str,
    pub fixed: bool,
    pub weights: &'static [i32],
    pub italic: bool,
}

/// Regular and Bold, the weights most families have.
const BOTH: &[i32] = &[400, 700];
/// One face, at the weight the family is named for.
const ONE: &[i32] = &[400];

/// A family of four faces: Regular and Bold, upright and italic.
const fn four(name: &'static str, fixed: bool) -> Family {
    Family { name, fixed, weights: BOTH, italic: true }
}

/// A family of one face, at `weight`, with an italic of its own.
const fn weight_of(name: &'static str, weight: &'static [i32]) -> Family {
    Family { name, fixed: false, weights: weight, italic: true }
}

/// A family of a single upright face.
const fn single(name: &'static str, fixed: bool, weights: &'static [i32]) -> Family {
    Family { name, fixed, weights, italic: false }
}

/// The families a program finds here, with the faces each answers for as
/// measured on Windows 11 24H2 for ANSI_CHARSET (user/gdi32_test's
/// `font_families`; a DEFAULT_CHARSET enumeration of a named family
/// repeats its faces once per character set it covers, which gala's one
/// Latin family does not). Listed by name: GDI promises no order.
///
/// Windows has families besides these which it does not enumerate for
/// ANSI_CHARSET -- its raster Courier, Terminal, Fixedsys, MS Sans Serif,
/// MS Serif and System, whose faces are OEM -- and gala, whose faces are
/// Latin, has none of them either; they are substituted (`SUBSTITUTED`).
pub static FAMILIES: &[Family] = &[
    four("Arial", false),
    single("Arial Black", false, &[900]),
    four("Calibri", false),
    weight_of("Calibri Light", &[300]),
    four("Cambria", false),
    single("Cambria Math", false, ONE),
    four("Candara", false),
    four("Comic Sans MS", false),
    four("Consolas", true),
    four("Constantia", false),
    four("Corbel", false),
    four("Courier New", true),
    four("Georgia", false),
    four("Helvetica", false),
    single("Impact", false, ONE),
    single("Lucida Console", true, ONE),
    single("Lucida Sans Unicode", false, ONE),
    four("Lucida Sans Typewriter", true),
    single("MS Shell Dlg", false, ONE),
    single("MS Shell Dlg 2", false, BOTH),
    single("MS UI Gothic", false, ONE),
    single("Microsoft Sans Serif", false, ONE),
    four("Palatino Linotype", false),
    four("Segoe UI", false),
    single("Segoe UI Emoji", false, ONE),
    weight_of("Segoe UI Light", &[300]),
    weight_of("Segoe UI Semibold", &[600]),
    weight_of("Segoe UI Semilight", &[350]),
    single("Segoe UI Symbol", false, ONE),
    single("Sylfaen", false, ONE),
    single("Tahoma", false, BOTH),
    four("Times", false),
    four("Times New Roman", false),
    four("Trebuchet MS", false),
    four("Verdana", false),
];

/// Names that ask for a fixed-pitch face which gala does not enumerate:
/// Windows' raster Courier, Terminal and Fixedsys (no ANSI faces, as
/// measured) and the Cascadia families, which it has only where a
/// terminal installed them. They realize as "Courier New", as a name no
/// family has does when it asks for fixed pitch.
const SUBSTITUTED: [&str; 5] =
    ["courier", "terminal", "fixedsys", "cascadia mono", "cascadia code"];

/// The family a name with no face of its own becomes: "Courier New" for
/// a request asking for fixed pitch, "Arial" otherwise. Both are
/// measured: on Windows an unknown name, and no name at all, realize as
/// "Arial", and either with FIXED_PITCH or FF_MODERN as "Courier New".
fn substitute(fixed: bool) -> &'static Family {
    let name = if fixed { "Courier New" } else { "Arial" };
    FAMILIES.iter().find(|f| f.name == name).unwrap_or(&FAMILIES[0])
}

/// Compares a face name (UTF-16, as a LOGFONT carries it) with a
/// family's, ignoring ASCII case, as GDI's font mapper does.
fn same_name(name: &[u16], family: &str) -> bool {
    let lower = |c: u16| if (0x41..=0x5a).contains(&c) { c + 0x20 } else { c };
    name.len() == family.len()
        && name
            .iter()
            .zip(family.as_bytes())
            .all(|(&a, &b)| lower(a) == u16::from(b.to_ascii_lowercase()))
}

/// The family a face name names, or None for one no family has.
pub fn family_named(name: &[u16]) -> Option<&'static Family> {
    FAMILIES.iter().find(|f| same_name(name, f.name))
}

/// The family a LOGFONT realizes with: the one its face name names, else
/// the substitute (`substitute`). A name of a fixed-pitch family gala
/// does not have, FIXED_PITCH (the low two bits 1) and FF_MODERN (the
/// family bits 0x30) all ask for fixed pitch.
pub fn family_of(f: &LogFont) -> &'static Family {
    let name = f.name();
    if let Some(family) = family_named(name) {
        return family;
    }
    let fixed = f.pitch_and_family & 3 == 1
        || f.pitch_and_family & 0xf0 == 0x30
        || SUBSTITUTED.iter().any(|&n| same_name(name, n));
    let family = substitute(fixed);
    note_substitution(name, family.name);
    family
}

/// A row naming a face a program asked for that no family here has, and
/// the family it was realized with -- what to read when a program lays
/// text out in the wrong face, or looks for a face it was given and does
/// not find it. First seen only, by a hash of the name over 64 buckets
/// (a collision drops a row, never the answer).
fn note_substitution(name: &[u16], family: &'static str) {
    static SEEN: AtomicU64 = AtomicU64::new(0);
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &c in name {
        hash = (hash ^ u64::from(c)).wrapping_mul(0x100_0000_01b3);
    }
    let mask = 1u64 << (hash % 64);
    if SEEN.fetch_or(mask, Ordering::Relaxed) & mask != 0 {
        return;
    }
    let mut what = [0u8; 96];
    let mut n = 0;
    let mut put = |bytes: &[u8]| {
        for &b in bytes {
            if n < what.len() {
                what[n] = b;
                n += 1;
            }
        }
    };
    put(b"gdi32.dll!font(");
    for &c in name.iter().take(32) {
        put(&[if (0x20..0x7f).contains(&c) { c as u8 } else { b'?' }]);
    }
    put(b" -> ");
    put(family.as_bytes());
    put(b")");
    crate::reached(&what[..n], name.len() as u64);
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

/// The weight a family answers an asked weight with: the nearest of its
/// faces' weights, the lighter where two are equally near. Measured on
/// Segoe UI, whose own faces are 400 and 700: it answers 350 with 400 and
/// 600 with 700, and the weights between them are the Semilight and
/// Semibold families' own.
pub fn snap_weight(weight: i32, weights: &[i32]) -> i32 {
    weights.iter().copied().min_by_key(|&w| ((w - weight).abs(), w)).unwrap_or(weight)
}

pub fn metrics(f: &LogFont) -> Metrics {
    let family = family_of(f);
    let weight = snap_weight(if f.weight == 0 { 400 } else { f.weight }, family.weights);
    if family.fixed {
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
    // From an em (lfHeight <= 0) the cell and the ascent are truncated,
    // not rounded: Segoe UI (whose winAscent, winDescent and unitsPerEm
    // Selawik shares: 2210, 514, 2048) at lfHeight -12 is 15 high with 12
    // above the baseline (Windows 11 25H2, 96 DPI; 12.95 and 15.96 before
    // truncation). Only that size is measured. A cell height (lfHeight > 0)
    // is kept as asked, so it rounds.
    let (ascent, height) = if f.height <= 0 {
        let ascent = (em64 * asc / (64 * upem)) as i32;
        (ascent, (em64 * (asc + desc) / (64 * upem)) as i32)
    } else {
        let ascent = round_div(em64 * asc, 64 * upem);
        (ascent, ascent + round_div(em64 * desc, 64 * upem))
    };
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

/// The font file that draws `f` (a Selawik weight), or None for the
/// fixed-pitch bitmap font, which has no file.
pub fn file(f: &LogFont) -> Option<&'static [u8]> {
    match metrics(f).kind {
        Kind::Selawik { face, .. } => Some(SELAWIK[face]),
        Kind::Mono { .. } => None,
    }
}

/// OS/2's xAvgCharWidth (the second field), in font units.
fn selawik_average(data: &[u8]) -> Option<i64> {
    Some(i64::from(s16(table(data, b"OS/2")?, 2)))
}

/// A table of a font file, by tag.
fn table<'a>(data: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
    let raw = ttf_parser::RawFace::parse(data, 0).ok()?;
    raw.table(ttf_parser::Tag::from_bytes(tag))
}

/// A signed 16-bit field of a table, or 0 where the table is too short.
fn s16(t: &[u8], at: usize) -> i32 {
    match (t.get(at), t.get(at + 1)) {
        (Some(&a), Some(&b)) => i32::from(i16::from_be_bytes([a, b])),
        _ => 0,
    }
}

/// An unsigned 16-bit field of a table, or 0 where the table is too short.
fn u16_at(t: &[u8], at: usize) -> i32 {
    match (t.get(at), t.get(at + 1)) {
        (Some(&a), Some(&b)) => i32::from(u16::from_be_bytes([a, b])),
        _ => 0,
    }
}

/// A field in font units at the realized em, rounded to the nearest
/// pixel (half away from zero). Every outline metric but the cap and x
/// heights is one of these.
fn scaled(units: i32, em: i32, upem: i32) -> i32 {
    if upem <= 0 {
        return 0;
    }
    let n = i64::from(units) * i64::from(em);
    let d = i64::from(upem);
    (if n < 0 { (2 * n - d) / (2 * d) } else { (2 * n + d) / (2 * d) }) as i32
}

/// What OUTLINETEXTMETRIC carries beyond the TEXTMETRIC, as the realized
/// font answers it (`user/gdi32_test`'s `outline_metrics`). Measured on
/// Arial, Segoe UI and Lucida Console at seven sizes, against those
/// files' own tables: every field here but the cap and x heights is a
/// field of the face's head, hhea, OS/2 or post table scaled to the em,
/// and those two are half the em and a quarter of it whatever the face
/// says (Arial's OS/2 carries 1467 and 1062 units -- 11 and 8 pixels at
/// an em of 16 -- and GDI answers 8 and 4).
pub struct Outline {
    pub panose: [u8; 10],
    pub fs_selection: u32,
    pub fs_type: u32,
    pub slope_rise: i32,
    pub slope_run: i32,
    pub italic_angle: i32,
    pub em_square: u32,
    pub ascent: i32,
    pub descent: i32,
    pub line_gap: u32,
    pub cap_em_height: u32,
    pub x_height: u32,
    /// Left, top, right and bottom, the way RECT holds them: the design
    /// box, so top is above the baseline (positive) and bottom below it.
    pub font_box: [i32; 4],
    pub mac_ascent: i32,
    pub mac_descent: i32,
    pub mac_line_gap: u32,
    pub min_ppem: u32,
    /// The size then the offset, x before y.
    pub subscript: [i32; 4],
    pub superscript: [i32; 4],
    /// The size then the position.
    pub strikeout: [i32; 2],
    pub underscore: [i32; 2],
    /// The family's name, the realized face's own style ("" for the
    /// raster faces, which carry no style or full name at all), and
    /// whether the realized slant is named in the face and full names.
    pub family: &'static str,
    pub style: &'static str,
    pub named_italic: bool,
}

/// The outline metrics of the font `f` realizes.
///
/// A slant gala synthesizes is reported the way Windows reports one it
/// simulates: in fsSelection (bit 0, the regular bit giving way to it)
/// and nowhere else -- an Impact italic, which has no italic face,
/// answers 0x01 with an italic angle of 0 and the upright slope. Where
/// the family enumerates italic faces of its own (`Family::italic`,
/// which EnumFontFamiliesExW answers with and the names below), the face
/// a program selected does exist as far as every other call is
/// concerned, so the slant is named there too -- and then the angle is
/// gala's own quarter-height shear, not a face's design angle.
pub fn outline(f: &LogFont) -> Outline {
    let family = family_of(f);
    let m = metrics(f);
    let em = m.em;
    // The style of the face the family realized: "Bold" only for the
    // heaviest of a family that has more than one weight. A family that
    // is itself a weight has the one face, and Windows names its style
    // "Regular" whatever that weight is (Arial Black answers "Regular"
    // with a tmWeight of 900).
    let heaviest = family.weights.iter().copied().max().unwrap_or(400);
    let style = if family.weights.len() > 1 && m.weight >= heaviest { "Bold" } else { "Regular" };
    let named_italic = m.italic && family.italic;
    let Some(data) = file(f) else { return raster(family, &m, named_italic) };
    let head = table(data, b"head").unwrap_or(&[]);
    let hhea = table(data, b"hhea").unwrap_or(&[]);
    let os2 = table(data, b"OS/2").unwrap_or(&[]);
    let post = table(data, b"post").unwrap_or(&[]);
    let upem = u16_at(head, 18).max(1);
    let s = |units: i32| scaled(units, em, upem);
    let mut panose = [0; 10];
    for (i, p) in panose.iter_mut().enumerate() {
        *p = os2.get(32 + i).copied().unwrap_or(0);
    }
    let selection = u16_at(os2, 62) as u32;
    // The shear is a quarter of the height above the baseline
    // (`Metrics::glyph`), which leans the face 14.04 degrees: an angle of
    // -140 tenths, and a slope of 4 up to 1 along.
    let (italic_angle, slope_rise, slope_run) =
        if named_italic { (-140, 4, 1) } else { (s16(post, 4) * 10, 1, 0) };
    Outline {
        panose,
        fs_selection: if m.italic { (selection & !0x40) | 0x01 } else { selection },
        fs_type: u16_at(os2, 8) as u32,
        slope_rise,
        slope_run,
        italic_angle,
        em_square: upem as u32,
        ascent: s(s16(os2, 68)),
        descent: s(s16(os2, 70)),
        line_gap: s(s16(os2, 72)).max(0) as u32,
        cap_em_height: ((em + 1) / 2).max(0) as u32,
        x_height: ((em + 2) / 4).max(0) as u32,
        font_box: [s(s16(head, 36)), s(s16(head, 42)), s(s16(head, 40)), s(s16(head, 38))],
        mac_ascent: s(s16(hhea, 4)),
        mac_descent: s(s16(hhea, 6)),
        mac_line_gap: s(s16(hhea, 8)).max(0) as u32,
        min_ppem: u16_at(head, 46) as u32,
        subscript: [s(s16(os2, 10)), s(s16(os2, 12)), s(s16(os2, 14)), s(s16(os2, 16))],
        superscript: [s(s16(os2, 18)), s(s16(os2, 20)), s(s16(os2, 22)), s(s16(os2, 24))],
        strikeout: [s(s16(os2, 26)), s(s16(os2, 28))],
        underscore: [s(s16(post, 10)), s(s16(post, 8))],
        family: family.name,
        style,
        named_italic,
    }
}

/// The answer a raster face gives, which gala's fixed-pitch families are
/// (they are drawn from bitmaps, with no file to read tables from): the
/// fields the bitmaps themselves have, and zero for the rest. Measured
/// on Windows' own MS Sans Serif and Small Fonts, which carry no style
/// or full name, no PANOSE beyond the first four numbers, no fsSelection
/// and no recommended PPEM, and whose em square is the em of the bitmap
/// strike in pixels rather than a design grid.
fn raster(family: &'static Family, m: &Metrics, named_italic: bool) -> Outline {
    let em = m.em;
    let descent = m.ascent - m.height;
    // A pixel of rule per 16 pixels of em, at the x height, which is
    // where MS Sans Serif puts it (1 and 4 at an em of 16).
    let rule = ((em + 8) / 16).max(1);
    Outline {
        panose: [2, 11, if m.weight >= 600 { 8 } else { 5 }, 9, 0, 0, 0, 0, 0, 0],
        fs_selection: if m.italic { 0x01 } else { 0 },
        fs_type: 0,
        slope_rise: 1,
        slope_run: 0,
        italic_angle: 0,
        em_square: em.max(0) as u32,
        ascent: m.ascent,
        descent,
        line_gap: 0,
        cap_em_height: ((em + 1) / 2).max(0) as u32,
        x_height: ((em + 2) / 4).max(0) as u32,
        font_box: [0, m.ascent, m.widest, descent],
        mac_ascent: m.ascent,
        mac_descent: descent,
        mac_line_gap: 0,
        min_ppem: 0,
        subscript: [0; 4],
        superscript: [0; 4],
        strikeout: [rule, ((em + 2) / 4).max(1)],
        underscore: [rule, -rule],
        family: family.name,
        style: "",
        named_italic,
    }
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
                if default_ignorable(c) && t.glyph_index(c).is_none() {
                    return 0;
                }
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
            Kind::Selawik { face, .. }
                if default_ignorable(c)
                    && ttf_parser::Face::parse(SELAWIK[face], 0)
                        .is_ok_and(|t| t.glyph_index(c).is_none()) => {}
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

/// Whether `c` is a default-ignorable code point (Unicode's
/// Default_Ignorable_Code_Point: format controls such as the bidi marks
/// and embeddings U+200E/F and U+202A-E, zero-width spaces and joiners,
/// variation selectors, tags), which the Unicode Standard (5.21) says to
/// render invisibly when a font has no glyph for them, as Windows' fonts
/// draw them (zero-width). The Hangul fillers and the soft hyphen, which
/// fonts draw, are left out.
pub fn default_ignorable(c: char) -> bool {
    matches!(
        c as u32,
        0x034f
            | 0x061c
            | 0x17b4..=0x17b5
            | 0x180b..=0x180f
            | 0x200b..=0x200f
            | 0x202a..=0x202e
            | 0x2060..=0x206f
            | 0xfe00..=0xfe0f
            | 0xfeff
            | 0xfff0..=0xfff8
            | 0x1bca0..=0x1bca3
            | 0x1d173..=0x1d17a
            | 0xe0000..=0xe0fff
    )
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

/// DEFAULT_CHARSET asks the font mapper to choose a character set; no
/// realized font keeps it.
pub const DEFAULT_CHARSET: u8 = 1;
/// The character set of the Latin code pages, which gala's faces cover.
pub const ANSI_CHARSET: u8 = 0;

/// The FONTSIGNATURE of a face that covers Latin 1 (bit 0 of the Unicode
/// ranges) and code page 1252 (bit 0 of the ANSI code pages): the
/// Unicode ranges' four words then the code pages' two. Selawik and Noto
/// Sans Mono are both Latin, so it is the signature of every font gala
/// realizes, and what EnumFontFamiliesExW reports for them.
pub const LATIN_SIGNATURE: [u32; 6] = [1, 0, 0, 0, 1, 0];

/// The character set the font is realized with, which GetTextCharsetInfo,
/// GetTextCharset and GetTextMetricsW's tmCharSet all report. A LOGFONT
/// asking for DEFAULT_CHARSET only asks the mapper to choose, so the
/// answer is the face's own: ANSI_CHARSET, gala's faces being Latin.
/// (Real Windows answers the character set of the machine's ANSI code
/// page here -- 178 where GetACP is 1256 -- because its mapper picks a
/// face to match it; gala has the one Latin family to pick.) A character
/// set the LOGFONT names is kept, as real Windows keeps it by
/// substituting a face that has it.
pub fn realized_charset(f: &LogFont) -> u8 {
    if f.char_set == DEFAULT_CHARSET { ANSI_CHARSET } else { f.char_set }
}

/// The Unicode ranges and code pages the font's face covers. A font
/// realized with a character set gala's faces do not cover has no
/// signature at all, which is how real Windows answers for a face that
/// carries none (its System and Terminal among them): all six words
/// zero.
pub fn signature(f: &LogFont) -> [u32; 6] {
    if realized_charset(f) == ANSI_CHARSET { LATIN_SIGNATURE } else { [0; 6] }
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
        char_set: realized_charset(f),
    }
}
