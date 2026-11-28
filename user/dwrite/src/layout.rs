//! Text layouts: a string, a format and a box, measured.
//!
//! A layout is the busiest object of Word's whole startup -- it asks for
//! a GDI-compatible one 414 times -- and it is two things at once. It is
//! a text format, carrying the same prefix (`format::Format`) and
//! answering the format's own 28 slots from a copy of the values it was
//! made with, so what the format is told afterwards does not reach it.
//! And it is a measurement: the lines the text broke into, the clusters
//! those lines are made of, the box the text fills inside the layout's
//! own, and the places in it a point or a position names.
//!
//! What "GDI-compatible" means here is a number, not a manner: the line
//! height, the baseline and the width are GDI's own `tmHeight`,
//! `tmAscent` and `GetTextExtentPoint32W` for the same face at the same
//! pixel size, which is why this file reproduces `gdi32`'s arithmetic
//! rather than scaling the design metrics -- `dwrite_test`'s step 68
//! asserts the two against each other, size by size, and is the guard
//! against the copies drifting apart. The pixels per DIP are a scale on
//! that pixel grid and nothing more: an em of `e` at `p` pixels per DIP
//! measures what GDI measures at `e * p` pixels, reported divided by `p`
//! again. The plain `CreateTextLayout`, which Word never calls, is the
//! unrounded one: the design metrics scaled by the em size, with the
//! line gap above the ascent belonging to the baseline.
//!
//! What a layout adds to a format is ranges. Every value a format holds
//! for the whole text a layout holds over a range of it and reports with
//! the run it found it in; what no range covers is the format's own over
//! a run that reaches `UINT32_MAX`. Twelve kinds of them, in `Ranges`,
//! each a sorted list of non-overlapping spans.
//!
//! Measuring is cached in `laid` and thrown away by `forget` whenever
//! anything it was computed from changes, which is every setter here and
//! the eight of the format's that change how text is laid out.
//!
//! Not there yet, each recorded as it is reached: drawing (`Draw` and
//! the renderer behind it), the alignments and the directions other than
//! the defaults, trimming (the options are kept and the ellipsis sign is
//! built, but no line is cut), line spacing methods other than DEFAULT,
//! bidirectional text (`max_bidi_depth` is always 1 and every level 0),
//! shaping (a cluster is a character), the family name of a range
//! choosing a face (the weight does), and every method of the layout's
//! later revisions.

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use alloc::vec::Vec;

use crate::format::{
    self, E_NOT_SUFFICIENT_BUFFER, STRETCH_MAX, STRETCH_MIN, STYLE_LAST, WEIGHT_MAX, WEIGHT_MIN,
};
use crate::{
    E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, HResult, Iid, S_OK, VTable, answered, font,
    guid, query, reached, vt,
};

pub const IID_TEXT_LAYOUT: Iid =
    guid(0x5373_7037, 0x6d14, 0x410b, [0x9b, 0xfe, 0x0b, 0x18, 0x2b, 0xb7, 0x09, 0x61]);
pub const IID_INLINE_OBJECT: Iid =
    guid(0x8339_fde3, 0x106f, 0x47ab, [0x83, 0x73, 0x1c, 0x62, 0x95, 0xeb, 0x10, 0xb3]);

/// DWRITE_WORD_WRAPPING_NO_WRAP, the one wrapping mode that is not a
/// kind of wrapping: the others all wrap and are taken as WRAP.
const NO_WRAP: u32 = 1;

/// Records a divergence the first time it is reached and not again, so
/// that a layout Word asks for hundreds of times leaves one row and not
/// hundreds (`missing` is kind 2, "unsupported").
macro_rules! gap {
    ($what:expr) => {{
        static ONCE: AtomicBool = AtomicBool::new(false);
        if !ONCE.swap(true, Ordering::Relaxed) {
            crate::missing($what, 0);
        }
    }};
}

// ---- What the layout reports through ---------------------------------------

/// DWRITE_TEXT_METRICS: the text's box inside the layout's, the layout's
/// own box reported back with it, and the two counts.
#[repr(C)]
#[allow(dead_code)]
struct TextMetrics {
    left: f32,
    top: f32,
    width: f32,
    width_with_trailing: f32,
    height: f32,
    layout_width: f32,
    layout_height: f32,
    max_bidi_depth: u32,
    line_count: u32,
}

/// DWRITE_LINE_METRICS: what one line holds and how tall it is.
#[repr(C)]
#[allow(dead_code)]
struct LineMetrics {
    length: u32,
    trailing_whitespace: u32,
    newline_length: u32,
    height: f32,
    baseline: f32,
    is_trimmed: i32,
}

/// DWRITE_CLUSTER_METRICS: an unbreakable piece of the text.
#[repr(C)]
#[allow(dead_code)]
struct ClusterMetrics {
    width: f32,
    length: u16,
    /// can_wrap_after, whitespace, newline, soft_hyphen, right_to_left,
    /// lowest bit first; the rest are padding.
    flags: u16,
}

/// DWRITE_OVERHANG_METRICS: how far the ink reaches past each side of
/// the layout box, positive outside.
#[repr(C)]
#[allow(dead_code)]
struct Overhang {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

/// DWRITE_HIT_TEST_METRICS: a place in the text and the box it fills.
#[repr(C)]
#[allow(dead_code)]
struct HitTestMetrics {
    text_position: u32,
    length: u32,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    bidi_level: u32,
    is_text: i32,
    is_trimmed: i32,
}

/// DWRITE_TEXT_RANGE: where a ranged value starts and how far it runs.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Range {
    start: u32,
    length: u32,
}

/// DWRITE_INLINE_OBJECT_METRICS: the box an inline object draws in.
#[repr(C)]
#[allow(dead_code)]
struct InlineMetrics {
    width: f32,
    height: f32,
    baseline: f32,
    supports_sideways: i32,
}

/// DWRITE_MATRIX, which a GDI-compatible layout takes and this one does
/// not look at (a transform other than the identity is recorded).
#[repr(C)]
pub struct Matrix {
    m11: f32,
    m12: f32,
    m21: f32,
    m22: f32,
    dx: f32,
    dy: f32,
}

// ---- The ranged values ----------------------------------------------------

/// A COM object a range holds a reference to, released with the span
/// that holds it. Cloning one is an AddRef, which is what splitting a
/// span in two owes it.
struct Held(*const c_void);

impl Clone for Held {
    fn clone(&self) -> Held {
        format::hold(self.0);
        Held(self.0)
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        format::drop_held(self.0);
    }
}

/// Takes a reference to `o` for a span to hold. NULL is a value like any
/// other here: a range may be told to hold nothing (measured).
fn held(o: *const c_void) -> Held {
    format::hold(o);
    Held(o)
}

/// One value over one run of the text.
struct Span<T> {
    start: u32,
    len: u32,
    value: T,
}

/// One kind of ranged value: the spans that carry it, sorted by where
/// they start and never overlapping.
struct Spans<T>(Vec<Span<T>>);

impl<T: Clone> Spans<T> {
    const fn new() -> Spans<T> {
        Spans(Vec::new())
    }

    /// The value at `pos` and the run it was found in. Where no span
    /// covers `pos` there is no value -- the format's own stands in --
    /// and the run reaches from the end of the span before it to the
    /// start of the one after, or to UINT32_MAX where there is none
    /// (measured, and what a program uses to walk the runs).
    fn at(&self, pos: u32) -> (Option<&T>, Range) {
        let mut floor = 0u32;
        for s in self.0.iter() {
            if pos < s.start {
                return (None, Range { start: floor, length: s.start - floor });
            }
            if pos - s.start < s.len {
                return (Some(&s.value), Range { start: s.start, length: s.len });
            }
            floor = s.start.saturating_add(s.len);
        }
        (None, Range { start: floor, length: u32::MAX - floor })
    }

    /// Lays `value` over the range, cutting back whatever was there. An
    /// empty range changes nothing and is S_OK (measured), and a range
    /// wholly past the text is kept as given -- the text is not consulted.
    fn set(&mut self, value: T, r: Range) -> HResult {
        if r.length == 0 {
            return S_OK;
        }
        let end = r.start.saturating_add(r.length);
        let mut kept: Vec<Span<T>> = Vec::new();
        // At most two of the old spans are cut in two, so one more than
        // the old count plus the new span is always enough.
        if kept.try_reserve(self.0.len() + 3).is_err() {
            return E_OUTOFMEMORY;
        }
        for s in self.0.drain(..) {
            let s_end = s.start.saturating_add(s.len);
            if s_end <= r.start || s.start >= end {
                kept.push(s);
                continue;
            }
            if s.start < r.start {
                kept.push(Span { start: s.start, len: r.start - s.start, value: s.value.clone() });
            }
            if s_end > end {
                kept.push(Span { start: end, len: s_end - end, value: s.value.clone() });
            }
        }
        kept.push(Span { start: r.start, len: end - r.start, value });
        kept.sort_unstable_by_key(|s| s.start);
        self.0 = kept;
        S_OK
    }
}

/// Every value a format holds for the whole text, held over ranges of it
/// instead. The four objects are held as they are given and not looked
/// at: what draws them looks at them, and nothing draws here.
struct Ranges {
    weight: Spans<u32>,
    style: Spans<u32>,
    stretch: Spans<u32>,
    size: Spans<f32>,
    underline: Spans<i32>,
    strikethrough: Spans<i32>,
    family: Spans<Vec<u16>>,
    locale: Spans<Vec<u16>>,
    collection: Spans<Held>,
    effect: Spans<Held>,
    object: Spans<Held>,
    typography: Spans<Held>,
}

impl Ranges {
    const fn new() -> Ranges {
        Ranges {
            weight: Spans::new(),
            style: Spans::new(),
            stretch: Spans::new(),
            size: Spans::new(),
            underline: Spans::new(),
            strikethrough: Spans::new(),
            family: Spans::new(),
            locale: Spans::new(),
            collection: Spans::new(),
            effect: Spans::new(),
            object: Spans::new(),
            typography: Spans::new(),
        }
    }
}

// ---- Measuring ------------------------------------------------------------

/// GDI's rounding: the nearest integer, halves away from zero for the
/// positive numbers this file has. The same arithmetic as `gdi32`'s
/// `font::round_div`, which step 68 holds this file to.
fn round_div(a: i64, b: i64) -> i64 {
    (2 * a + b) / (2 * b)
}

/// Whether `c` is a default-ignorable code point, which a font without a
/// glyph for it renders as nothing rather than as the missing-glyph box.
/// The same table as `gdi32`'s `font::default_ignorable`: the two are
/// separate crates and there is nowhere shared to put it, so step 68 --
/// which measures this file's widths against gdi32's -- is the guard
/// against them drifting apart.
fn default_ignorable(c: char) -> bool {
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

/// One face at one size, parsed once and measured from. A layout keeps
/// one of these at a time while it walks the text, so a run of
/// characters in the same font parses the face once.
struct Measure {
    face: usize,
    size: f32,
    /// The pixels per DIP of a GDI-compatible layout, or None for the
    /// unrounded one.
    ppd: Option<f32>,
    f: ttf_parser::Face<'static>,
    upem: i64,
    /// The pixel size GDI would be asked for, which is what everything
    /// here is rounded on.
    em_px: i64,
    height: f32,
    baseline: f32,
}

impl Measure {
    fn new(face: usize, size: f32, ppd: Option<f32>) -> Option<Measure> {
        let f = font::face(face)?;
        let (upem, ascent, descent, gap) = font::design(&f);
        let (upem, asc, desc, gap) =
            (i64::from(upem), i64::from(ascent), i64::from(descent), i64::from(gap));
        if upem <= 0 {
            return None;
        }
        // GDI's own two numbers, from a negative LOGFONT height: the
        // ascent and the height are the em scaled by the face's Windows
        // ascent and descent and truncated, with no line gap. gdi32
        // scales both sides by 64 first, which divides out again.
        let (em_px, height, baseline) = match ppd {
            Some(p) => {
                let em_px = ((size * p + 0.5) as i64).clamp(1, 1024);
                (em_px, (em_px * (asc + desc) / upem) as f32 / p, (em_px * asc / upem) as f32 / p)
            }
            // The unrounded line, the line gap above the ascent
            // belonging to the baseline. Written in this order because
            // the test's own arithmetic is in this order.
            None => (
                0,
                (asc + desc + gap) as f32 * size / upem as f32,
                (asc + gap) as f32 * size / upem as f32,
            ),
        };
        Some(Measure { face, size, ppd, f, upem, em_px, height, baseline })
    }

    /// How far one character advances the pen. A character the face has
    /// no glyph for is drawn as its missing-glyph box, which is what
    /// `'?'` maps to, unless it is default-ignorable and draws as
    /// nothing at all.
    fn advance(&self, c: char) -> f32 {
        let t = &self.f;
        if default_ignorable(c) && t.glyph_index(c).is_none() {
            return 0.0;
        }
        let g = t.glyph_index(c).or_else(|| t.glyph_index('?')).unwrap_or_default();
        let adv = i64::from(t.glyph_hor_advance(g).unwrap_or(0));
        match self.ppd {
            Some(p) => round_div(self.em_px * adv, self.upem) as f32 / p,
            None => adv as f32 * self.size / self.upem as f32,
        }
    }
}

const WRAP_AFTER: u16 = 1;
const WHITESPACE: u16 = 2;
const NEWLINE: u16 = 4;

/// One cluster: the smallest piece of text that cannot be broken, which
/// without shaping is one character (a surrogate pair is the two code
/// units it is).
#[derive(Clone, Copy)]
struct Cluster {
    /// Where it starts, in UTF-16 code units from the start of the text.
    start: u32,
    len: u16,
    flags: u16,
    /// The character itself, kept so that a CRLF can be told from a lone
    /// LF when a line reports its newline's length.
    c: char,
    width: f32,
    /// The line height and baseline of the font this cluster is in; a
    /// line takes the tallest of its own.
    height: f32,
    baseline: f32,
}

impl Cluster {
    fn wrap_after(&self) -> bool {
        self.flags & WRAP_AFTER != 0
    }

    fn whitespace(&self) -> bool {
        self.flags & WHITESPACE != 0
    }

    fn newline(&self) -> bool {
        self.flags & NEWLINE != 0
    }
}

/// One line: the clusters it holds, what it reports about them, and
/// where it sits.
struct Line {
    /// The clusters `first..last`.
    first: usize,
    last: usize,
    start: u32,
    len: u32,
    trailing: u32,
    newline_len: u32,
    /// Without the trailing whitespace, which is what the text is
    /// measured at, and with it, which is the other width.
    width: f32,
    width_all: f32,
    height: f32,
    baseline: f32,
    top: f32,
}

/// A whole measurement, thrown away by `forget` when anything it was
/// computed from changes.
struct Laid {
    clusters: Vec<Cluster>,
    lines: Vec<Line>,
    width: f32,
    width_all: f32,
    height: f32,
}

/// The width of `cs[a..b]` as the text is measured: the trailing
/// whitespace is dropped, and what is left is added left to right.
fn fitting(cs: &[Cluster], a: usize, b: usize) -> f32 {
    let mut end = b;
    while end > a && cs[end - 1].whitespace() {
        end -= 1;
    }
    cs[a..end].iter().map(|c| c.width).sum()
}

/// Appends one line over the clusters `a..b`, which may be empty (the
/// line a trailing newline leaves, and the line a layout of no text at
/// all still has), in which case the format's own font gives its height.
fn push_line(
    cs: &[Cluster],
    lines: &mut Vec<Line>,
    base: &Measure,
    a: usize,
    b: usize,
) -> Option<()> {
    if lines.try_reserve(1).is_err() {
        return None;
    }
    let (start, len) = if a < b {
        let s = cs[a].start;
        (s, cs[b - 1].start + u32::from(cs[b - 1].len) - s)
    } else if a > 0 {
        (cs[a - 1].start + u32::from(cs[a - 1].len), 0)
    } else {
        (0, 0)
    };
    let mut trailing = 0u32;
    let mut k = b;
    while k > a && cs[k - 1].whitespace() {
        trailing += u32::from(cs[k - 1].len);
        k -= 1;
    }
    // A break belongs to the line it ends, and a CRLF is the two
    // characters it is -- of which only the LF is the newline.
    let newline_len = if b > a && cs[b - 1].newline() {
        if cs[b - 1].c == '\n' && b - 1 > a && cs[b - 2].c == '\r' { 2 } else { 1 }
    } else {
        0
    };
    let (mut height, mut baseline) = (base.height, base.baseline);
    if a < b {
        height = 0.0;
        baseline = 0.0;
        for c in &cs[a..b] {
            if c.height > height {
                height = c.height;
            }
            if c.baseline > baseline {
                baseline = c.baseline;
            }
        }
    }
    lines.push(Line {
        first: a,
        last: b,
        start,
        len,
        trailing,
        newline_len,
        width: fitting(cs, a, b),
        width_all: cs[a..b].iter().map(|c| c.width).sum(),
        height,
        baseline,
        top: 0.0,
    });
    Some(())
}

/// Measures the whole text: the clusters, then the lines they break
/// into. None where an allocation or the font failed, which every method
/// that needs a measurement answers E_OUTOFMEMORY to.
fn lay(l: &Layout) -> Option<Laid> {
    let own = l.fmt.values;
    let mut base = Measure::new(font::nearest(own.weight), own.size, l.ppd)?;
    if own.text_alignment != 0 || own.paragraph_alignment != 0 {
        gap!(b"dwrite.dll!IDWriteTextLayout: text is laid out at the box's origin only");
    }
    if own.reading != 0 || own.flow != 0 {
        gap!(b"dwrite.dll!IDWriteTextLayout: text flows left to right and down only");
    }
    if own.spacing_method != 0 {
        gap!(b"dwrite.dll!IDWriteTextLayout: the line spacing method is the font's own only");
    }
    if own.trimming[0] != 0 {
        gap!(b"dwrite.dll!IDWriteTextLayout: no line is trimmed");
    }
    if own.wrapping > NO_WRAP {
        gap!(b"dwrite.dll!IDWriteTextLayout: every wrapping mode but NO_WRAP wraps greedily");
    }

    let n = l.text.len();
    let mut clusters: Vec<Cluster> = Vec::new();
    if clusters.try_reserve(n).is_err() {
        return None;
    }
    // The pen, from the start of the paragraph, which is all a tab stop
    // is measured from.
    let mut x = 0.0f32;
    let mut i = 0usize;
    while i < n {
        let (c, units) = decode(&l.text, i);
        let start = i as u32;
        let next = i + usize::from(units);
        // A CR is a newline only where no LF follows it: a CRLF breaks
        // once, after the LF.
        let newline = c == '\n' || (c == '\r' && l.text.get(next) != Some(&0x000a));
        let whitespace = matches!(c, ' ' | '\t' | '\r' | '\n');

        let size = l.ranges.size.at(start).0.copied().unwrap_or(own.size);
        let weight = l.ranges.weight.at(start).0.copied().unwrap_or(own.weight);
        if l.ranges.family.at(start).0.is_some() {
            gap!(b"dwrite.dll!IDWriteTextLayout::SetFontFamilyName: the weight picks the face");
        }
        let face = font::nearest(weight);
        if base.face != face || base.size != size {
            base = Measure::new(face, size, l.ppd)?;
        }

        let width = match c {
            // A break takes no width of its own: the face has no glyph
            // for either character, and the missing-glyph box is not
            // what a line end draws (measured).
            '\r' | '\n' => 0.0,
            '\t' => {
                let stop = own.tab_stop;
                let steps = (x / stop) as i32;
                (steps + 1) as f32 * stop - x
            }
            _ => base.advance(c),
        };
        let mut flags = 0u16;
        if whitespace {
            flags |= WHITESPACE;
        }
        if newline {
            flags |= NEWLINE | WRAP_AFTER;
        } else if c == ' ' || c == '\t' {
            flags |= WRAP_AFTER;
        }
        clusters.push(Cluster {
            start,
            len: units,
            flags,
            c,
            width,
            height: base.height,
            baseline: base.baseline,
        });
        x = if newline { 0.0 } else { x + width };
        i = next;
    }
    // The end of the text is a place a line may end as surely as a
    // space is (measured).
    if let Some(last) = clusters.last_mut() {
        last.flags |= WRAP_AFTER;
    }

    let nc = clusters.len();
    let limit = if own.wrapping == NO_WRAP { f32::INFINITY } else { l.max_width };
    let mut lines: Vec<Line> = Vec::new();
    let mut pos = 0usize;
    while pos < nc {
        // Greedy: the last opportunity that still fits, stopping at a
        // mandatory break.
        let mut end = 0usize;
        let mut i = pos;
        while i < nc {
            if clusters[i].wrap_after() {
                if fitting(&clusters, pos, i + 1) > limit {
                    break;
                }
                end = i + 1;
                if clusters[i].newline() {
                    break;
                }
            }
            i += 1;
        }
        if end == 0 {
            // Nowhere to break that fits: break between characters
            // rather than overflow, but never past the first
            // opportunity, and always take at least one.
            let first_break = (pos..nc).find(|&k| clusters[k].wrap_after()).map_or(nc, |k| k + 1);
            end = pos + 1;
            while end < first_break && fitting(&clusters, pos, end + 1) <= limit {
                end += 1;
            }
        }
        push_line(&clusters, &mut lines, &base, pos, end)?;
        pos = end;
    }
    // No text, or text that ends in a break, still has a last line.
    if nc == 0 || clusters[nc - 1].newline() {
        push_line(&clusters, &mut lines, &base, nc, nc)?;
    }

    let (mut width, mut width_all, mut height) = (0.0f32, 0.0f32, 0.0f32);
    for line in lines.iter_mut() {
        line.top = height;
        if line.width > width {
            width = line.width;
        }
        if line.width_all > width_all {
            width_all = line.width_all;
        }
        height += line.height;
    }
    Some(Laid { clusters, lines, width, width_all, height })
}

/// The character at `i` and how many code units it took. A lone
/// surrogate is not a character and stands in as U+FFFD.
fn decode(text: &[u16], i: usize) -> (char, u16) {
    let u = text[i];
    if (0xd800..0xdc00).contains(&u)
        && let Some(&low) = text.get(i + 1)
        && (0xdc00..0xe000).contains(&low)
    {
        let c = 0x1_0000 + (u32::from(u - 0xd800) << 10) + u32::from(low - 0xdc00);
        return (char::from_u32(c).unwrap_or(char::REPLACEMENT_CHARACTER), 2);
    }
    (char::from_u32(u32::from(u)).unwrap_or(char::REPLACEMENT_CHARACTER), 1)
}

// ---- The layout -----------------------------------------------------------

/// A text layout: a text format's own prefix, then the string, the box
/// and the ranges. `#[repr(C)]` with the format first is what lets the
/// 28 format methods in `format.rs` read this object as one of theirs.
#[repr(C)]
struct Layout {
    fmt: format::Format,
    text: Vec<u16>,
    /// The format's two strings, copied so that the prefix can point at
    /// them: NUL-terminated, written once and never moved.
    family: Vec<u16>,
    locale: Vec<u16>,
    max_width: f32,
    max_height: f32,
    /// The pixels per DIP of a GDI-compatible layout, or None.
    ppd: Option<f32>,
    ranges: Ranges,
    laid: Option<Laid>,
}

impl Drop for Layout {
    fn drop(&mut self) {
        // The values are a copy of the format's, and the two objects in
        // them are references of this layout's own.
        format::drop_held(self.fmt.values.sign);
        format::drop_held(self.fmt.values.collection);
    }
}

/// Throws the measurement away, which every setter owes.
fn forget(this: *const c_void) {
    // SAFETY: one of our layouts.
    unsafe { (*this.cast_mut().cast::<Layout>()).laid = None };
}

/// Measures the layout if it has not been measured since it last
/// changed, then hands the measurement to `f`. `refused` is the answer
/// where measuring failed, which is only ever an allocation or a font
/// that would not parse.
fn with_laid<R>(this: *const c_void, refused: R, f: impl FnOnce(&Layout, &Laid) -> R) -> R {
    // SAFETY: one of our layouts, and the cache is filled through the
    // pointer before anything borrows it.
    unsafe {
        let p = this.cast_mut().cast::<Layout>();
        if (*p).laid.is_none() {
            (*p).laid = lay(&*p);
        }
        match (*p).laid.as_ref() {
            Some(laid) => f(&*p, laid),
            None => refused,
        }
    }
}

/// A NUL-terminated copy of a string, which is what the layout holds its
/// family and locale names as.
fn copy_of(from: (*const u16, usize)) -> Option<Vec<u16>> {
    let mut v: Vec<u16> = Vec::new();
    v.try_reserve(from.1 + 1).ok()?;
    for i in 0..from.1 {
        // SAFETY: a string of `from.1` characters.
        v.push(unsafe { *from.0.add(i) });
    }
    v.push(0);
    Some(v)
}

static LAYOUT_VTBL: VTable<89> = VTable([
    layout_qi as *const (),
    layout_add_ref as *const (),
    layout_release as *const (),
    // 3..=10: the format's setters, which change how the text is laid
    // out, so each throws the measurement away.
    layout_SetTextAlignment as *const (),
    layout_SetParagraphAlignment as *const (),
    layout_SetWordWrapping as *const (),
    layout_SetReadingDirection as *const (),
    layout_SetFlowDirection as *const (),
    layout_SetIncrementalTabStop as *const (),
    layout_SetTrimming as *const (),
    layout_SetLineSpacing as *const (),
    // 11..=27: the format's getters, which read the prefix this object
    // begins with and need nothing of their own.
    format::format_GetTextAlignment as *const (),
    format::format_GetParagraphAlignment as *const (),
    format::format_GetWordWrapping as *const (),
    format::format_GetReadingDirection as *const (),
    format::format_GetFlowDirection as *const (),
    format::format_GetIncrementalTabStop as *const (),
    format::format_GetTrimming as *const (),
    format::format_GetLineSpacing as *const (),
    format::format_GetFontCollection as *const (),
    format::format_GetFontFamilyNameLength as *const (),
    format::format_GetFontFamilyName as *const (),
    format::format_GetFontWeight as *const (),
    format::format_GetFontStyle as *const (),
    format::format_GetFontStretch as *const (),
    format::format_GetFontSize as *const (),
    format::format_GetLocaleNameLength as *const (),
    format::format_GetLocaleName as *const (),
    // 28..=66: the layout's own.
    layout_SetMaxWidth as *const (),
    layout_SetMaxHeight as *const (),
    layout_SetFontCollection as *const (),
    layout_SetFontFamilyName as *const (),
    layout_SetFontWeight as *const (),
    layout_SetFontStyle as *const (),
    layout_SetFontStretch as *const (),
    layout_SetFontSize as *const (),
    layout_SetUnderline as *const (),
    layout_SetStrikethrough as *const (),
    layout_SetDrawingEffect as *const (),
    layout_SetInlineObject as *const (),
    layout_SetTypography as *const (),
    layout_SetLocaleName as *const (),
    layout_GetMaxWidth as *const (),
    layout_GetMaxHeight as *const (),
    layout_GetFontCollection as *const (),
    layout_GetFontFamilyNameLength as *const (),
    layout_GetFontFamilyName as *const (),
    layout_GetFontWeight as *const (),
    layout_GetFontStyle as *const (),
    layout_GetFontStretch as *const (),
    layout_GetFontSize as *const (),
    layout_GetUnderline as *const (),
    layout_GetStrikethrough as *const (),
    layout_GetDrawingEffect as *const (),
    layout_GetInlineObject as *const (),
    layout_GetTypography as *const (),
    layout_GetLocaleNameLength as *const (),
    layout_GetLocaleName as *const (),
    layout_Draw as *const (),
    layout_GetLineMetrics as *const (),
    layout_GetMetrics as *const (),
    layout_GetOverhangMetrics as *const (),
    layout_GetClusterMetrics as *const (),
    layout_DetermineMinWidth as *const (),
    layout_HitTestPoint as *const (),
    layout_HitTestTextPosition as *const (),
    layout_HitTestTextRange as *const (),
    // 67..=88: the later revisions, each refused and recorded.
    layout1_SetPairKerning as *const (),
    layout1_GetPairKerning as *const (),
    layout1_SetCharacterSpacing as *const (),
    layout1_GetCharacterSpacing as *const (),
    layout2_GetMetrics as *const (),
    layout2_SetVerticalGlyphOrientation as *const (),
    layout2_GetVerticalGlyphOrientation as *const (),
    layout2_SetLastLineWrapping as *const (),
    layout2_GetLastLineWrapping as *const (),
    layout2_SetOpticalAlignment as *const (),
    layout2_GetOpticalAlignment as *const (),
    layout2_SetFontFallback as *const (),
    layout2_GetFontFallback as *const (),
    layout3_InvalidateLayout as *const (),
    layout3_SetLineSpacing as *const (),
    layout3_GetLineSpacing as *const (),
    layout3_GetLineMetrics as *const (),
    layout4_SetFontAxisValues as *const (),
    layout4_GetFontAxisValueCount as *const (),
    layout4_GetFontAxisValues as *const (),
    layout4_GetAutomaticFontAxes as *const (),
    layout4_SetAutomaticFontAxes as *const (),
]);

// ---- The three the factory answers ----------------------------------------

/// IDWriteFactory::CreateTextLayout(text, len, format, maxWidth,
/// maxHeight, &layout) (18): the unrounded layout, which Word does not
/// ask for.
pub extern "system" fn factory_CreateTextLayout(
    _this: *const c_void,
    text: *const u16,
    len: u32,
    format: *const c_void,
    max_width: f32,
    max_height: f32,
    out: *mut *const c_void,
) -> HResult {
    let r = make(text, len, format, max_width, max_height, None, out);
    answered(b"IDWriteFactory::CreateTextLayout", r);
    r
}

/// IDWriteFactory::CreateGdiCompatibleTextLayout(text, len, format,
/// layoutWidth, layoutHeight, pixelsPerDip, transform, useGdiNatural,
/// &layout) (19): the layout Word asks for, measured on GDI's pixel
/// grid. The pixels per DIP are taken as they come -- zero and a
/// negative are S_OK (measured) -- and the transform and
/// `useGdiNatural` are taken and recorded.
#[allow(clippy::too_many_arguments)]
pub extern "system" fn factory_CreateGdiCompatibleTextLayout(
    _this: *const c_void,
    text: *const u16,
    len: u32,
    format: *const c_void,
    max_width: f32,
    max_height: f32,
    pixels_per_dip: f32,
    transform: *const Matrix,
    use_gdi_natural: i32,
    out: *mut *const c_void,
) -> HResult {
    if !transform.is_null() {
        // SAFETY: the caller's transform.
        let m = unsafe { &*transform };
        if m.m11 != 1.0 || m.m12 != 0.0 || m.m21 != 0.0 || m.m22 != 1.0 {
            gap!(b"dwrite.dll!IDWriteFactory::CreateGdiCompatibleTextLayout: the transform is not applied");
        }
    }
    if use_gdi_natural != 0 {
        gap!(b"dwrite.dll!IDWriteFactory::CreateGdiCompatibleTextLayout: useGdiNatural measures as GDI classic");
    }
    let mut ppd = pixels_per_dip;
    if !ppd.is_finite() || ppd <= 0.0 {
        gap!(b"dwrite.dll!IDWriteFactory::CreateGdiCompatibleTextLayout: a pixel scale of zero or less measures as one");
        ppd = 1.0;
    }
    let r = make(text, len, format, max_width, max_height, Some(ppd), out);
    answered(b"IDWriteFactory::CreateGdiCompatibleTextLayout", r);
    r
}

/// A layout of its own for the caller. The string is copied, the
/// format's values and strings are copied (what the format is told
/// afterwards does not reach the layout -- measured), and the two
/// objects in those values get a reference of the layout's own.
fn make(
    text: *const u16,
    len: u32,
    format: *const c_void,
    max_width: f32,
    max_height: f32,
    ppd: Option<f32>,
    out: *mut *const c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's output.
    unsafe { *out = core::ptr::null() };
    if !format.is_null() && !format::ours(format) {
        gap!(
            b"dwrite.dll!IDWriteFactory::CreateTextLayout: a text format of another implementation"
        );
        return E_INVALIDARG;
    }
    let refused = format.is_null()
        || text.is_null()
        || max_width.is_nan()
        || max_width < 0.0
        || max_height.is_nan()
        || max_height < 0.0;
    if refused {
        return E_INVALIDARG;
    }

    let n = len as usize;
    let mut copy: Vec<u16> = Vec::new();
    if copy.try_reserve(n).is_err() {
        return E_OUTOFMEMORY;
    }
    for i in 0..n {
        // SAFETY: the caller's string, which has `len` characters.
        copy.push(unsafe { *text.add(i) });
    }
    let (family, locale) = format::strings(format);
    let (Some(family), Some(locale)) = (copy_of(family), copy_of(locale)) else {
        return E_OUTOFMEMORY;
    };

    let lay = core::alloc::Layout::new::<Layout>();
    // SAFETY: a non-zero-sized layout.
    let p = unsafe { alloc::alloc::alloc(lay) }.cast::<Layout>();
    if p.is_null() {
        return E_OUTOFMEMORY;
    }
    let values = format::values(format);
    format::hold(values.sign);
    format::hold(values.collection);
    // SAFETY: fresh memory for one Layout. The prefix's two string
    // pointers are filled in afterwards, once the vectors are in place
    // and will not move again.
    unsafe {
        p.write(Layout {
            fmt: format::Format {
                vtbl: vt(&LAYOUT_VTBL),
                refs: AtomicU32::new(1),
                values,
                family: (core::ptr::null(), 0),
                locale: (core::ptr::null(), 0),
            },
            text: copy,
            family,
            locale,
            max_width,
            max_height,
            ppd,
            ranges: Ranges::new(),
            laid: None,
        });
        (*p).fmt.family = ((*p).family.as_ptr(), (*p).family.len() - 1);
        (*p).fmt.locale = ((*p).locale.as_ptr(), (*p).locale.len() - 1);
        *out = p.cast();
    }
    S_OK
}

/// IDWriteFactory::CreateEllipsisTrimmingSign(format, &sign) (20): the
/// inline object a trimmed line ends with, as wide as the ellipsis it
/// draws in the format's own font.
pub extern "system" fn factory_CreateEllipsisTrimmingSign(
    _this: *const c_void,
    format: *const c_void,
    out: *mut *const c_void,
) -> HResult {
    let r = make_sign(format, out);
    answered(b"IDWriteFactory::CreateEllipsisTrimmingSign", r);
    r
}

fn make_sign(format: *const c_void, out: *mut *const c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's output.
    unsafe { *out = core::ptr::null() };
    if format.is_null() || !format::ours(format) {
        return E_INVALIDARG;
    }
    let values = format::values(format);
    let Some(m) = Measure::new(font::nearest(values.weight), values.size, None) else {
        return crate::E_FAIL;
    };
    // The ellipsis where the font has one, and the three full stops it
    // stands for where it has not.
    let width = if m.f.glyph_index('\u{2026}').is_some() {
        m.advance('\u{2026}')
    } else {
        3.0 * m.advance('.')
    };
    let lay = core::alloc::Layout::new::<Sign>();
    // SAFETY: a non-zero-sized layout.
    let p = unsafe { alloc::alloc::alloc(lay) }.cast::<Sign>();
    if p.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: fresh memory for one Sign.
    unsafe {
        p.write(Sign { vtbl: vt(&SIGN_VTBL), refs: AtomicU32::new(1), width });
        *out = p.cast();
    }
    S_OK
}

// ---- IUnknown -------------------------------------------------------------

/// QueryInterface: a layout answers for itself as a text layout and as
/// the text format it is (Windows hands out an object of its own for the
/// format, which the test does not hold us to). Its own later revisions
/// are refused, as the format's are.
extern "system" fn layout_qi(
    this: *const c_void,
    riid: *const Iid,
    out: *mut *const c_void,
) -> HResult {
    let r = query(this, &[IID_TEXT_LAYOUT, format::IID_TEXT_FORMAT], riid, out);
    if r == S_OK {
        layout_add_ref(this);
    }
    r
}

extern "system" fn layout_add_ref(this: *const c_void) -> u32 {
    // SAFETY: one of our layouts.
    unsafe { (*this.cast::<Layout>()).fmt.refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

extern "system" fn layout_release(this: *const c_void) -> u32 {
    // SAFETY: one of our layouts.
    let left = unsafe { (*this.cast::<Layout>()).fmt.refs.fetch_sub(1, Ordering::AcqRel) } - 1;
    if left == 0 {
        // SAFETY: the last reference is gone, and the allocation is
        // `make`'s, of one Layout.
        unsafe {
            let p = this.cast_mut().cast::<Layout>();
            core::ptr::drop_in_place(p);
            alloc::alloc::dealloc(p.cast(), core::alloc::Layout::new::<Layout>());
        }
    }
    left
}

// ---- 3..=10: the format's setters, measured again afterwards ---------------

/// The eight values a format holds that change how the text is laid out:
/// the format answers each of them for this object, and the layout
/// throws its measurement away afterwards.
macro_rules! remeasured {
    ($($name:ident($($arg:ident: $ty:ty),*) = $inner:path;)*) => {
        $(
            extern "system" fn $name(this: *const c_void $(, $arg: $ty)*) -> HResult {
                let r = $inner(this $(, $arg)*);
                if r == S_OK {
                    forget(this);
                }
                r
            }
        )*
    };
}

remeasured! {
    layout_SetTextAlignment(value: u32) = format::format_SetTextAlignment;
    layout_SetParagraphAlignment(value: u32) = format::format_SetParagraphAlignment;
    layout_SetWordWrapping(value: u32) = format::format_SetWordWrapping;
    layout_SetReadingDirection(value: u32) = format::format_SetReadingDirection;
    layout_SetFlowDirection(value: u32) = format::format_SetFlowDirection;
    layout_SetIncrementalTabStop(stop: f32) = format::format_SetIncrementalTabStop;
    layout_SetTrimming(options: *const [u32; 3], sign: *const c_void) = format::format_SetTrimming;
    layout_SetLineSpacing(method: u32, spacing: f32, baseline: f32) = format::format_SetLineSpacing;
}

// ---- 28..=29: the box -----------------------------------------------------

/// SetMaxWidth(width) (28) / SetMaxHeight(height) (29): a negative side
/// is refused and leaves the one it has (measured).
extern "system" fn layout_SetMaxWidth(this: *const c_void, width: f32) -> HResult {
    reached!(b"IDWriteTextLayout::SetMaxWidth");
    if width.is_nan() || width < 0.0 {
        return E_INVALIDARG;
    }
    // SAFETY: one of our layouts.
    unsafe { (*this.cast_mut().cast::<Layout>()).max_width = width };
    forget(this);
    S_OK
}

extern "system" fn layout_SetMaxHeight(this: *const c_void, height: f32) -> HResult {
    reached!(b"IDWriteTextLayout::SetMaxHeight");
    if height.is_nan() || height < 0.0 {
        return E_INVALIDARG;
    }
    // SAFETY: one of our layouts.
    unsafe { (*this.cast_mut().cast::<Layout>()).max_height = height };
    forget(this);
    S_OK
}

extern "system" fn layout_GetMaxWidth(this: *const c_void) -> f32 {
    reached!(b"IDWriteTextLayout::GetMaxWidth");
    // SAFETY: one of our layouts.
    unsafe { (*this.cast::<Layout>()).max_width }
}

extern "system" fn layout_GetMaxHeight(this: *const c_void) -> f32 {
    reached!(b"IDWriteTextLayout::GetMaxHeight");
    // SAFETY: one of our layouts.
    unsafe { (*this.cast::<Layout>()).max_height }
}

// ---- 30..=41: the ranged setters ------------------------------------------

/// The layout behind `this`, borrowed for one method.
fn layout(this: *const c_void) -> &'static mut Layout {
    // SAFETY: one of our layouts, which outlives the call.
    unsafe { &mut *this.cast_mut().cast::<Layout>() }
}

extern "system" fn layout_SetFontCollection(
    this: *const c_void,
    collection: *const c_void,
    r: Range,
) -> HResult {
    reached!(b"IDWriteTextLayout::SetFontCollection");
    let hr = layout(this).ranges.collection.set(held(collection), r);
    forget(this);
    hr
}

extern "system" fn layout_SetFontFamilyName(
    this: *const c_void,
    family: *const u16,
    r: Range,
) -> HResult {
    reached!(b"IDWriteTextLayout::SetFontFamilyName");
    if family.is_null() {
        return E_INVALIDARG;
    }
    let Some(name) = copy_of((family, format::wide_len(family))) else {
        return E_OUTOFMEMORY;
    };
    let hr = layout(this).ranges.family.set(name, r);
    forget(this);
    hr
}

extern "system" fn layout_SetFontWeight(this: *const c_void, weight: u32, r: Range) -> HResult {
    reached!(b"IDWriteTextLayout::SetFontWeight");
    if !(WEIGHT_MIN..=WEIGHT_MAX).contains(&weight) {
        return E_INVALIDARG;
    }
    let hr = layout(this).ranges.weight.set(weight, r);
    forget(this);
    hr
}

extern "system" fn layout_SetFontStyle(this: *const c_void, style: u32, r: Range) -> HResult {
    reached!(b"IDWriteTextLayout::SetFontStyle");
    if style > STYLE_LAST {
        return E_INVALIDARG;
    }
    let hr = layout(this).ranges.style.set(style, r);
    forget(this);
    hr
}

extern "system" fn layout_SetFontStretch(this: *const c_void, stretch: u32, r: Range) -> HResult {
    reached!(b"IDWriteTextLayout::SetFontStretch");
    if !(STRETCH_MIN..=STRETCH_MAX).contains(&stretch) {
        return E_INVALIDARG;
    }
    let hr = layout(this).ranges.stretch.set(stretch, r);
    forget(this);
    hr
}

extern "system" fn layout_SetFontSize(this: *const c_void, size: f32, r: Range) -> HResult {
    reached!(b"IDWriteTextLayout::SetFontSize");
    if size.is_nan() || size <= 0.0 {
        return E_INVALIDARG;
    }
    let hr = layout(this).ranges.size.set(size, r);
    forget(this);
    hr
}

extern "system" fn layout_SetUnderline(this: *const c_void, on: i32, r: Range) -> HResult {
    reached!(b"IDWriteTextLayout::SetUnderline");
    layout(this).ranges.underline.set(on, r)
}

extern "system" fn layout_SetStrikethrough(this: *const c_void, on: i32, r: Range) -> HResult {
    reached!(b"IDWriteTextLayout::SetStrikethrough");
    layout(this).ranges.strikethrough.set(on, r)
}

extern "system" fn layout_SetDrawingEffect(
    this: *const c_void,
    effect: *const c_void,
    r: Range,
) -> HResult {
    reached!(b"IDWriteTextLayout::SetDrawingEffect");
    layout(this).ranges.effect.set(held(effect), r)
}

extern "system" fn layout_SetInlineObject(
    this: *const c_void,
    object: *const c_void,
    r: Range,
) -> HResult {
    reached!(b"IDWriteTextLayout::SetInlineObject");
    if !object.is_null() {
        gap!(
            b"dwrite.dll!IDWriteTextLayout::SetInlineObject: the object takes no room in the line"
        );
    }
    let hr = layout(this).ranges.object.set(held(object), r);
    forget(this);
    hr
}

extern "system" fn layout_SetTypography(
    this: *const c_void,
    typography: *const c_void,
    r: Range,
) -> HResult {
    reached!(b"IDWriteTextLayout::SetTypography");
    if !typography.is_null() {
        gap!(b"dwrite.dll!IDWriteTextLayout::SetTypography: the features do not reach the measurement");
    }
    layout(this).ranges.typography.set(held(typography), r)
}

extern "system" fn layout_SetLocaleName(
    this: *const c_void,
    locale: *const u16,
    r: Range,
) -> HResult {
    reached!(b"IDWriteTextLayout::SetLocaleName");
    if locale.is_null() || !format::locale_shaped(locale) {
        return E_INVALIDARG;
    }
    let Some(name) = copy_of((locale, format::wide_len(locale))) else {
        return E_OUTOFMEMORY;
    };
    layout(this).ranges.locale.set(name, r)
}

// ---- 44..=57: the ranged getters ------------------------------------------

/// A ranged value and the run it was found in, the format's own standing
/// in where no span covers the position. The run is optional: a program
/// that does not care passes NULL (measured).
fn give<T: Copy>(spans: &Spans<T>, own: T, pos: u32, out: *mut T, run: *mut Range) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let (value, r) = spans.at(pos);
    // SAFETY: the caller's outputs.
    unsafe {
        *out = value.copied().unwrap_or(own);
        if !run.is_null() {
            *run = r;
        }
    }
    S_OK
}

/// The same for an object a range holds, which the caller gets a
/// reference to. NULL is a value a range can hold, and holding nothing
/// is what no span at all means too.
fn give_held(
    spans: &Spans<Held>,
    own: *const c_void,
    pos: u32,
    out: *mut *const c_void,
    run: *mut Range,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let (value, r) = spans.at(pos);
    let object = match value {
        Some(h) => h.0,
        None => own,
    };
    format::hold(object);
    // SAFETY: the caller's outputs.
    unsafe {
        *out = object;
        if !run.is_null() {
            *run = r;
        }
    }
    S_OK
}

/// A ranged string and its run, the format's own standing in.
fn string_at(
    spans: &Spans<Vec<u16>>,
    own: (*const u16, usize),
    pos: u32,
) -> ((*const u16, usize), Range) {
    let (value, r) = spans.at(pos);
    match value {
        Some(s) => ((s.as_ptr(), s.len() - 1), r),
        None => (own, r),
    }
}

extern "system" fn layout_GetFontCollection(
    this: *const c_void,
    pos: u32,
    out: *mut *const c_void,
    run: *mut Range,
) -> HResult {
    reached!(b"IDWriteTextLayout::GetFontCollection");
    let l = layout(this);
    let own = l.fmt.values.collection;
    // The format's own NULL means the system's collection, as it does
    // for a format (`format_GetFontCollection`).
    let own = if own.is_null() { font::collection().unwrap_or(core::ptr::null()) } else { own };
    give_held(&l.ranges.collection, own, pos, out, run)
}

extern "system" fn layout_GetFontFamilyNameLength(
    this: *const c_void,
    pos: u32,
    out: *mut u32,
    run: *mut Range,
) -> HResult {
    reached!(b"IDWriteTextLayout::GetFontFamilyNameLength");
    if out.is_null() {
        return E_POINTER;
    }
    let l = layout(this);
    let (name, r) = string_at(&l.ranges.family, l.fmt.family, pos);
    // SAFETY: the caller's outputs.
    unsafe {
        *out = name.1 as u32;
        if !run.is_null() {
            *run = r;
        }
    }
    S_OK
}

extern "system" fn layout_GetFontFamilyName(
    this: *const c_void,
    pos: u32,
    name: *mut u16,
    size: u32,
    run: *mut Range,
) -> HResult {
    reached!(b"IDWriteTextLayout::GetFontFamilyName");
    let l = layout(this);
    let (from, r) = string_at(&l.ranges.family, l.fmt.family, pos);
    let hr = format::give_string(from, name, size);
    if !run.is_null() {
        // SAFETY: the caller's output.
        unsafe { *run = r };
    }
    hr
}

extern "system" fn layout_GetFontWeight(
    this: *const c_void,
    pos: u32,
    out: *mut u32,
    run: *mut Range,
) -> HResult {
    reached!(b"IDWriteTextLayout::GetFontWeight");
    let l = layout(this);
    give(&l.ranges.weight, l.fmt.values.weight, pos, out, run)
}

extern "system" fn layout_GetFontStyle(
    this: *const c_void,
    pos: u32,
    out: *mut u32,
    run: *mut Range,
) -> HResult {
    reached!(b"IDWriteTextLayout::GetFontStyle");
    let l = layout(this);
    give(&l.ranges.style, l.fmt.values.style, pos, out, run)
}

extern "system" fn layout_GetFontStretch(
    this: *const c_void,
    pos: u32,
    out: *mut u32,
    run: *mut Range,
) -> HResult {
    reached!(b"IDWriteTextLayout::GetFontStretch");
    let l = layout(this);
    give(&l.ranges.stretch, l.fmt.values.stretch, pos, out, run)
}

extern "system" fn layout_GetFontSize(
    this: *const c_void,
    pos: u32,
    out: *mut f32,
    run: *mut Range,
) -> HResult {
    reached!(b"IDWriteTextLayout::GetFontSize");
    let l = layout(this);
    give(&l.ranges.size, l.fmt.values.size, pos, out, run)
}

extern "system" fn layout_GetUnderline(
    this: *const c_void,
    pos: u32,
    out: *mut i32,
    run: *mut Range,
) -> HResult {
    reached!(b"IDWriteTextLayout::GetUnderline");
    give(&layout(this).ranges.underline, 0, pos, out, run)
}

extern "system" fn layout_GetStrikethrough(
    this: *const c_void,
    pos: u32,
    out: *mut i32,
    run: *mut Range,
) -> HResult {
    reached!(b"IDWriteTextLayout::GetStrikethrough");
    give(&layout(this).ranges.strikethrough, 0, pos, out, run)
}

extern "system" fn layout_GetDrawingEffect(
    this: *const c_void,
    pos: u32,
    out: *mut *const c_void,
    run: *mut Range,
) -> HResult {
    reached!(b"IDWriteTextLayout::GetDrawingEffect");
    give_held(&layout(this).ranges.effect, core::ptr::null(), pos, out, run)
}

extern "system" fn layout_GetInlineObject(
    this: *const c_void,
    pos: u32,
    out: *mut *const c_void,
    run: *mut Range,
) -> HResult {
    reached!(b"IDWriteTextLayout::GetInlineObject");
    give_held(&layout(this).ranges.object, core::ptr::null(), pos, out, run)
}

extern "system" fn layout_GetTypography(
    this: *const c_void,
    pos: u32,
    out: *mut *const c_void,
    run: *mut Range,
) -> HResult {
    reached!(b"IDWriteTextLayout::GetTypography");
    give_held(&layout(this).ranges.typography, core::ptr::null(), pos, out, run)
}

extern "system" fn layout_GetLocaleNameLength(
    this: *const c_void,
    pos: u32,
    out: *mut u32,
    run: *mut Range,
) -> HResult {
    reached!(b"IDWriteTextLayout::GetLocaleNameLength");
    if out.is_null() {
        return E_POINTER;
    }
    let l = layout(this);
    let (name, r) = string_at(&l.ranges.locale, l.fmt.locale, pos);
    // SAFETY: the caller's outputs.
    unsafe {
        *out = name.1 as u32;
        if !run.is_null() {
            *run = r;
        }
    }
    S_OK
}

extern "system" fn layout_GetLocaleName(
    this: *const c_void,
    pos: u32,
    name: *mut u16,
    size: u32,
    run: *mut Range,
) -> HResult {
    reached!(b"IDWriteTextLayout::GetLocaleName");
    let l = layout(this);
    let (from, r) = string_at(&l.ranges.locale, l.fmt.locale, pos);
    let hr = format::give_string(from, name, size);
    if !run.is_null() {
        // SAFETY: the caller's output.
        unsafe { *run = r };
    }
    hr
}

// ---- 58..=63: the measurements --------------------------------------------

/// Draw(context, renderer, x, y) (58): nothing draws yet.
extern "system" fn layout_Draw(
    _this: *const c_void,
    _context: *const c_void,
    _renderer: *const c_void,
    _x: f32,
    _y: f32,
) -> HResult {
    crate::missing(b"dwrite.dll!IDWriteTextLayout::Draw", 0);
    E_NOTIMPL
}

/// GetLineMetrics(&lines, max, &count) (59): the count is written
/// whatever happens, and a buffer that cannot hold them all is
/// E_NOT_SUFFICIENT_BUFFER (measured).
extern "system" fn layout_GetLineMetrics(
    this: *const c_void,
    out: *mut LineMetrics,
    max: u32,
    count: *mut u32,
) -> HResult {
    reached!(b"IDWriteTextLayout::GetLineMetrics");
    if count.is_null() {
        return E_POINTER;
    }
    with_laid(this, E_OUTOFMEMORY, |_, laid| {
        let n = laid.lines.len();
        // SAFETY: the caller's count.
        unsafe { *count = n as u32 };
        if out.is_null() || (max as usize) < n {
            return E_NOT_SUFFICIENT_BUFFER;
        }
        for (i, line) in laid.lines.iter().enumerate() {
            // SAFETY: the caller's buffer, which holds `n`.
            unsafe {
                out.add(i).write(LineMetrics {
                    length: line.len,
                    trailing_whitespace: line.trailing,
                    newline_length: line.newline_len,
                    height: line.height,
                    baseline: line.baseline,
                    is_trimmed: 0,
                });
            }
        }
        S_OK
    })
}

/// GetMetrics(&metrics) (60): the text's box inside the layout's, which
/// sits at its origin until the alignments are answered.
extern "system" fn layout_GetMetrics(this: *const c_void, out: *mut TextMetrics) -> HResult {
    reached!(b"IDWriteTextLayout::GetMetrics");
    if out.is_null() {
        return E_POINTER;
    }
    with_laid(this, E_OUTOFMEMORY, |l, laid| {
        // SAFETY: the caller's output.
        unsafe {
            out.write(TextMetrics {
                left: 0.0,
                top: 0.0,
                width: laid.width,
                width_with_trailing: laid.width_all,
                height: laid.height,
                layout_width: l.max_width,
                layout_height: l.max_height,
                max_bidi_depth: 1,
                line_count: laid.lines.len() as u32,
            });
        }
        S_OK
    })
}

/// GetOverhangMetrics(&overhang) (61): how far the ink reaches past each
/// side of the layout box, positive outside. gala has no ink box -- it
/// does not rasterize here -- so this is the text's own box against the
/// layout's, which is inside it on the left and the top by construction.
extern "system" fn layout_GetOverhangMetrics(this: *const c_void, out: *mut Overhang) -> HResult {
    reached!(b"IDWriteTextLayout::GetOverhangMetrics");
    if out.is_null() {
        return E_POINTER;
    }
    gap!(b"dwrite.dll!IDWriteTextLayout::GetOverhangMetrics: the text's box, not the ink's");
    with_laid(this, E_OUTOFMEMORY, |l, laid| {
        // SAFETY: the caller's output.
        unsafe {
            out.write(Overhang {
                left: 0.0,
                top: 0.0,
                right: laid.width - l.max_width,
                bottom: laid.height - l.max_height,
            });
        }
        S_OK
    })
}

/// GetClusterMetrics(&clusters, max, &count) (62): one per character
/// here, since nothing shapes yet. No clusters at all is S_OK with a
/// count of zero and not a buffer too small (measured).
extern "system" fn layout_GetClusterMetrics(
    this: *const c_void,
    out: *mut ClusterMetrics,
    max: u32,
    count: *mut u32,
) -> HResult {
    reached!(b"IDWriteTextLayout::GetClusterMetrics");
    if count.is_null() {
        return E_POINTER;
    }
    with_laid(this, E_OUTOFMEMORY, |_, laid| {
        let n = laid.clusters.len();
        // SAFETY: the caller's count.
        unsafe { *count = n as u32 };
        if n == 0 {
            return S_OK;
        }
        if out.is_null() || (max as usize) < n {
            return E_NOT_SUFFICIENT_BUFFER;
        }
        for (i, c) in laid.clusters.iter().enumerate() {
            // SAFETY: the caller's buffer, which holds `n`.
            unsafe {
                out.add(i).write(ClusterMetrics {
                    width: c.width,
                    length: c.len,
                    flags: c.flags & (WRAP_AFTER | WHITESPACE | NEWLINE),
                });
            }
        }
        S_OK
    })
}

/// DetermineMinWidth(&width) (63): the narrowest box the text would fit
/// in, which is the widest piece of it that cannot be broken.
extern "system" fn layout_DetermineMinWidth(this: *const c_void, out: *mut f32) -> HResult {
    reached!(b"IDWriteTextLayout::DetermineMinWidth");
    if out.is_null() {
        return E_POINTER;
    }
    with_laid(this, E_OUTOFMEMORY, |_, laid| {
        let cs = &laid.clusters;
        let (mut widest, mut a) = (0.0f32, 0usize);
        for i in 0..cs.len() {
            if cs[i].wrap_after() {
                let w = fitting(cs, a, i + 1);
                if w > widest {
                    widest = w;
                }
                a = i + 1;
            }
        }
        if a < cs.len() {
            let w = fitting(cs, a, cs.len());
            if w > widest {
                widest = w;
            }
        }
        // SAFETY: the caller's output.
        unsafe { *out = widest };
        S_OK
    })
}

// ---- 64..=66: hit testing -------------------------------------------------

/// The box one cluster of a line fills, and where it starts in the text.
fn cluster_box(laid: &Laid, line: &Line, i: usize, left: f32) -> HitTestMetrics {
    let c = &laid.clusters[i];
    HitTestMetrics {
        text_position: c.start,
        length: u32::from(c.len),
        left,
        top: line.top,
        width: c.width,
        height: line.height,
        bidi_level: 0,
        is_text: 1,
        is_trimmed: 0,
    }
}

/// HitTestPoint(x, y, &trailing, &inside, &metrics) (64): which
/// character a point lands on, whether it landed on the far half of it
/// and whether it landed on the text at all.
extern "system" fn layout_HitTestPoint(
    this: *const c_void,
    x: f32,
    y: f32,
    trailing: *mut i32,
    inside: *mut i32,
    out: *mut HitTestMetrics,
) -> HResult {
    reached!(b"IDWriteTextLayout::HitTestPoint");
    if trailing.is_null() || inside.is_null() || out.is_null() {
        return E_POINTER;
    }
    with_laid(this, E_OUTOFMEMORY, |_, laid| {
        // The line whose band holds y, or the nearest one.
        let mut li = 0usize;
        for (k, line) in laid.lines.iter().enumerate() {
            li = k;
            if y < line.top + line.height {
                break;
            }
        }
        let line = &laid.lines[li];
        let mut found = None;
        let mut acc = 0.0f32;
        for i in line.first..line.last {
            let w = laid.clusters[i].width;
            if x < acc + w {
                found = Some((i, acc));
                break;
            }
            acc += w;
        }
        let (mut hit, mut past) = match found {
            Some((i, left)) => (cluster_box(laid, line, i, left), false),
            // Past the end of the line, which is the last character
            // trailing -- or the line's own start where it is empty.
            None if line.last > line.first => {
                let i = line.last - 1;
                (cluster_box(laid, line, i, acc - laid.clusters[i].width), true)
            }
            None => (
                HitTestMetrics {
                    text_position: line.start,
                    length: 0,
                    left: 0.0,
                    top: line.top,
                    width: 0.0,
                    height: line.height,
                    bidi_level: 0,
                    is_text: 0,
                    is_trimmed: 0,
                },
                true,
            ),
        };
        if !past {
            past = x >= hit.left + hit.width / 2.0;
        }
        hit.is_trimmed = 0;
        let within = x >= 0.0
            && x <= line.width_all
            && y >= line.top
            && y < line.top + line.height
            && line.last > line.first;
        // SAFETY: the caller's outputs.
        unsafe {
            *trailing = i32::from(past);
            *inside = i32::from(within);
            out.write(hit);
        }
        S_OK
    })
}

/// HitTestTextPosition(position, trailing, &x, &y, &metrics) (65): where
/// a character sits, the leading edge unless the trailing one was asked
/// for.
extern "system" fn layout_HitTestTextPosition(
    this: *const c_void,
    position: u32,
    trailing: i32,
    x: *mut f32,
    y: *mut f32,
    out: *mut HitTestMetrics,
) -> HResult {
    reached!(b"IDWriteTextLayout::HitTestTextPosition");
    if x.is_null() || y.is_null() || out.is_null() {
        return E_POINTER;
    }
    with_laid(this, E_OUTOFMEMORY, |_, laid| {
        let mut li = laid.lines.len() - 1;
        for (k, line) in laid.lines.iter().enumerate() {
            if position < line.start + line.len {
                li = k;
                break;
            }
        }
        let line = &laid.lines[li];
        let mut hit = HitTestMetrics {
            text_position: line.start + line.len,
            length: 0,
            left: line.width_all,
            top: line.top,
            width: 0.0,
            height: line.height,
            bidi_level: 0,
            is_text: 0,
            is_trimmed: 0,
        };
        let mut acc = 0.0f32;
        for i in line.first..line.last {
            let c = &laid.clusters[i];
            if position < c.start + u32::from(c.len) {
                hit = cluster_box(laid, line, i, acc);
                break;
            }
            acc += c.width;
        }
        // SAFETY: the caller's outputs.
        unsafe {
            *x = if trailing != 0 { hit.left + hit.width } else { hit.left };
            *y = hit.top;
            out.write(hit);
        }
        S_OK
    })
}

/// HitTestTextRange(position, length, originX, originY, &boxes, max,
/// &count) (66): the boxes a range of characters fills, one per line it
/// reaches into. The count is written whatever happens.
#[allow(clippy::too_many_arguments)]
extern "system" fn layout_HitTestTextRange(
    this: *const c_void,
    position: u32,
    length: u32,
    origin_x: f32,
    origin_y: f32,
    out: *mut HitTestMetrics,
    max: u32,
    count: *mut u32,
) -> HResult {
    reached!(b"IDWriteTextLayout::HitTestTextRange");
    if count.is_null() {
        return E_POINTER;
    }
    with_laid(this, E_OUTOFMEMORY, |_, laid| {
        let end = position.saturating_add(length);
        let mut n = 0u32;
        for line in laid.lines.iter() {
            let (ls, le) = (line.start, line.start + line.len);
            let a = if position > ls { position } else { ls };
            let b = if end < le { end } else { le };
            if a >= b {
                continue;
            }
            let (mut left, mut width, mut first) = (0.0f32, 0.0f32, a);
            let mut seen = false;
            let mut acc = 0.0f32;
            for i in line.first..line.last {
                let c = &laid.clusters[i];
                if c.start + u32::from(c.len) <= a {
                    acc += c.width;
                    continue;
                }
                if c.start >= b {
                    break;
                }
                if !seen {
                    left = acc;
                    first = c.start;
                    seen = true;
                }
                width += c.width;
                acc += c.width;
            }
            if n < max && !out.is_null() {
                // SAFETY: the caller's buffer, which holds `max`.
                unsafe {
                    out.add(n as usize).write(HitTestMetrics {
                        text_position: first,
                        length: b - a,
                        left: left + origin_x,
                        top: line.top + origin_y,
                        width,
                        height: line.height,
                        bidi_level: 0,
                        is_text: 1,
                        is_trimmed: 0,
                    });
                }
            }
            n += 1;
        }
        // SAFETY: the caller's count.
        unsafe { *count = n };
        if n > max || (n > 0 && out.is_null()) {
            return E_NOT_SUFFICIENT_BUFFER;
        }
        S_OK
    })
}

// ---- 67..=88: the later revisions -----------------------------------------

format::revision_not_impl! {
    layout1_SetPairKerning(i32, Range) -> HResult
        as "IDWriteTextLayout1::SetPairKerning" = E_NOTIMPL;
    layout1_GetPairKerning(u32, *mut i32, *mut Range) -> HResult
        as "IDWriteTextLayout1::GetPairKerning" = E_NOTIMPL;
    layout1_SetCharacterSpacing(f32, f32, f32, Range) -> HResult
        as "IDWriteTextLayout1::SetCharacterSpacing" = E_NOTIMPL;
    layout1_GetCharacterSpacing(u32, *mut f32, *mut f32, *mut f32, *mut Range) -> HResult
        as "IDWriteTextLayout1::GetCharacterSpacing" = E_NOTIMPL;
    layout2_GetMetrics(*mut c_void) -> HResult
        as "IDWriteTextLayout2::GetMetrics" = E_NOTIMPL;
    layout2_SetVerticalGlyphOrientation(u32) -> HResult
        as "IDWriteTextLayout2::SetVerticalGlyphOrientation" = E_NOTIMPL;
    layout2_GetVerticalGlyphOrientation() -> u32
        as "IDWriteTextLayout2::GetVerticalGlyphOrientation" = 0;
    layout2_SetLastLineWrapping(i32) -> HResult
        as "IDWriteTextLayout2::SetLastLineWrapping" = E_NOTIMPL;
    layout2_GetLastLineWrapping() -> i32
        as "IDWriteTextLayout2::GetLastLineWrapping" = 0;
    layout2_SetOpticalAlignment(u32) -> HResult
        as "IDWriteTextLayout2::SetOpticalAlignment" = E_NOTIMPL;
    layout2_GetOpticalAlignment() -> u32
        as "IDWriteTextLayout2::GetOpticalAlignment" = 0;
    layout2_SetFontFallback(*const c_void) -> HResult
        as "IDWriteTextLayout2::SetFontFallback" = E_NOTIMPL;
    layout2_GetFontFallback(*mut *const c_void) -> HResult
        as "IDWriteTextLayout2::GetFontFallback" = E_NOTIMPL;
    layout3_InvalidateLayout() -> HResult
        as "IDWriteTextLayout3::InvalidateLayout" = E_NOTIMPL;
    layout3_SetLineSpacing(*const c_void) -> HResult
        as "IDWriteTextLayout3::SetLineSpacing" = E_NOTIMPL;
    layout3_GetLineSpacing(*mut c_void) -> HResult
        as "IDWriteTextLayout3::GetLineSpacing" = E_NOTIMPL;
    layout3_GetLineMetrics(*mut c_void, u32, *mut u32) -> HResult
        as "IDWriteTextLayout3::GetLineMetrics" = E_NOTIMPL;
    layout4_SetFontAxisValues(*const c_void, u32, Range) -> HResult
        as "IDWriteTextLayout4::SetFontAxisValues" = E_NOTIMPL;
    layout4_GetFontAxisValueCount(u32) -> u32
        as "IDWriteTextLayout4::GetFontAxisValueCount" = 0;
    layout4_GetFontAxisValues(u32, *mut c_void, u32) -> HResult
        as "IDWriteTextLayout4::GetFontAxisValues" = E_NOTIMPL;
    layout4_GetAutomaticFontAxes(u32) -> u32
        as "IDWriteTextLayout4::GetAutomaticFontAxes" = 0;
    layout4_SetAutomaticFontAxes(u32, Range) -> HResult
        as "IDWriteTextLayout4::SetAutomaticFontAxes" = E_NOTIMPL;
}

// ---- IDWriteInlineObject: the ellipsis trimming sign ----------------------

/// The object a trimmed line ends with: as wide as the ellipsis it
/// draws, of no height of its own (it sits on the baseline it is given)
/// and breaking neither way.
#[repr(C)]
struct Sign {
    vtbl: *const (),
    refs: AtomicU32,
    width: f32,
}

static SIGN_VTBL: VTable<7> = VTable([
    sign_qi as *const (),
    sign_add_ref as *const (),
    sign_release as *const (),
    sign_Draw as *const (),
    sign_GetMetrics as *const (),
    sign_GetOverhangMetrics as *const (),
    sign_GetBreakConditions as *const (),
]);

extern "system" fn sign_qi(
    this: *const c_void,
    riid: *const Iid,
    out: *mut *const c_void,
) -> HResult {
    let r = query(this, &[IID_INLINE_OBJECT], riid, out);
    if r == S_OK {
        sign_add_ref(this);
    }
    r
}

extern "system" fn sign_add_ref(this: *const c_void) -> u32 {
    // SAFETY: one of our signs.
    unsafe { (*this.cast::<Sign>()).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

extern "system" fn sign_release(this: *const c_void) -> u32 {
    // SAFETY: one of our signs.
    let left = unsafe { (*this.cast::<Sign>()).refs.fetch_sub(1, Ordering::AcqRel) } - 1;
    if left == 0 {
        // SAFETY: the last reference is gone, and the allocation is
        // `make_sign`'s, of one Sign.
        unsafe {
            let p = this.cast_mut().cast::<Sign>();
            core::ptr::drop_in_place(p);
            alloc::alloc::dealloc(p.cast(), core::alloc::Layout::new::<Sign>());
        }
    }
    left
}

extern "system" fn sign_Draw(
    _this: *const c_void,
    _context: *const c_void,
    _renderer: *const c_void,
    _x: f32,
    _y: f32,
    _sideways: i32,
    _rtl: i32,
    _effect: *const c_void,
) -> HResult {
    crate::missing(b"dwrite.dll!IDWriteInlineObject::Draw", 0);
    E_NOTIMPL
}

extern "system" fn sign_GetMetrics(this: *const c_void, out: *mut InlineMetrics) -> HResult {
    reached!(b"IDWriteInlineObject::GetMetrics");
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: one of our signs, and the caller's output.
    unsafe {
        out.write(InlineMetrics {
            width: (*this.cast::<Sign>()).width,
            height: 0.0,
            baseline: 0.0,
            supports_sideways: 0,
        });
    }
    S_OK
}

extern "system" fn sign_GetOverhangMetrics(_this: *const c_void, out: *mut Overhang) -> HResult {
    reached!(b"IDWriteInlineObject::GetOverhangMetrics");
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's output.
    unsafe { out.write(Overhang { left: 0.0, top: 0.0, right: 0.0, bottom: 0.0 }) };
    S_OK
}

/// GetBreakConditions(&before, &after) (6): NEUTRAL both ways, which is
/// what an ellipsis is -- it neither invites a break nor forbids one.
extern "system" fn sign_GetBreakConditions(
    _this: *const c_void,
    before: *mut u32,
    after: *mut u32,
) -> HResult {
    reached!(b"IDWriteInlineObject::GetBreakConditions");
    if before.is_null() || after.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's outputs.
    unsafe {
        *before = 0;
        *after = 0;
    }
    S_OK
}
