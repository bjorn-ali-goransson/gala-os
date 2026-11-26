//! Text analysis (`IDWriteTextAnalyzer`): what a program asks the
//! factory for before it lays a line out, and where Word's startup
//! stopped without it -- CreateTextAnalyzer refused 13 ms before its
//! error box, with nothing else between.
//!
//! One static analyzer, because a text analyzer holds no state: Windows
//! makes a fresh object per call, which no caller here can tell apart,
//! as nothing in this DLL is ever freed. It answers
//! IDWriteTextAnalyzer, 1 and 2 with itself -- one pointer, three
//! revisions, and there is no revision 3 -- and refuses anything else.
//! The policy at `FACTORY_IIDS`, where a revision answered but not
//! implemented is worse for a program than one refused, does not reach
//! here: a program that is refused a factory revision takes a fallback,
//! while a program refused its text analyzer has none, which is exactly
//! what killed Word. An analyzer whose unimplemented methods record
//! their own names is strictly better, and the ledger then says which
//! method a program wanted next.
//!
//! What the four `Analyze*` methods report is measured on Windows 11
//! 24H2 (dwrite_test step 12):
//!
//! * Script itemization runs over code points, not units. A run holds
//!   while characters repeat its script or join (space, digits and
//!   ASCII punctuation, which carry no script of their own and extend
//!   whatever run they fall in): `Hello world 123` is one Latin run,
//!   and `Hi <arabic> 4<han>` is three, the space and the digit going
//!   with the Arabic before them. Only the three scripts measured have
//!   numbers here (Latin 49, Arabic 3, Han 33); no public list gives
//!   the rest, so an unnamed one joins instead of inventing a number,
//!   and says so in the ledger.
//! * A break is allowed between two characters when the earlier one is
//!   a space, or either one is an ideograph; the condition before the
//!   range's first character may not break and the one after its last
//!   may. A space carries isWhitespace. Inside a surrogate pair there
//!   is no break.
//! * Bidi: one run at level 0. Resolving levels (the Unicode
//!   bidirectional algorithm) is not done -- a *stopgap*, in ROADMAP:
//!   right-to-left text leaves a ledger row rather than a wrong level.
//!   The same for a hard line break, which Windows marks MUST_BREAK and
//!   this breaks like a space.
//! * Number substitution is a silent success: S_OK with the sink never
//!   called, even over digits. That is the contract, not a gap.
//!
//! `GetGlyphs` maps each code point to the glyph the face itself gives
//! it, one glyph per character with the identity cluster map, every
//! glyph a cluster start, justification 4 for a space and 2 for
//! everything else. `GetGlyphPlacements` scales each glyph's own design
//! advance by the em size and offsets nothing. Neither applies a
//! feature, a substitution or a kern, and the GDI-compatible placements
//! refuse rather than answer the non-GDI numbers.

use core::ffi::c_void;

use crate::{
    E_FAIL, E_INVALIDARG, E_NOTIMPL, E_POINTER, HResult, Iid, S_OK, VTable, answered, guid,
    missing, query, reached, vt,
};

pub const IID_ANALYZER: Iid =
    guid(0xb7e6_163e, 0x7f46, 0x43b4, [0x84, 0xb3, 0xe4, 0xe6, 0x24, 0x9c, 0x36, 0x5d]);
pub const IID_ANALYZER1: Iid =
    guid(0x80da_d800, 0xe21f, 0x4e83, [0x96, 0xce, 0xbf, 0xcc, 0xe5, 0x00, 0xdb, 0x7c]);
pub const IID_ANALYZER2: Iid =
    guid(0x553a_9ff3, 0x5693, 0x4df7, [0xb5, 0x2b, 0x74, 0x80, 0x6f, 0x7f, 0x2e, 0xb9]);

const ANALYZER_IIDS: [Iid; 3] = [IID_ANALYZER, IID_ANALYZER1, IID_ANALYZER2];

/// HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER), which GetGlyphs
/// answers when the glyph buffers are too small for the text.
const E_INSUFFICIENT_BUFFER: HResult = 0x8007_007a_u32 as i32;

/// DWRITE_SCRIPT_ANALYSIS. Eight bytes and not four:
/// DWRITE_SCRIPT_SHAPES is a C enum, so `shapes` sits at offset 4.
#[repr(C)]
pub struct ScriptAnalysis {
    script: u16,
    shapes: u32,
}

/// DWRITE_SCRIPT_SHAPES_DEFAULT.
const SHAPES_DEFAULT: u32 = 0;

/// DWRITE_GLYPH_OFFSET.
#[repr(C)]
pub struct GlyphOffset {
    advance: f32,
    ascender: f32,
}

/// DWRITE_BREAK_CONDITION_CAN_BREAK.
const CAN_BREAK: u8 = 1;
/// DWRITE_BREAK_CONDITION_MAY_NOT_BREAK.
const MAY_NOT_BREAK: u8 = 2;
/// DWRITE_LINE_BREAKPOINT::isWhitespace, bit 4 of the byte.
const IS_WHITESPACE: u8 = 1 << 4;
/// DWRITE_SHAPING_GLYPH_PROPERTIES::isClusterStart, bit 4 of the word.
const IS_CLUSTER_START: u16 = 1 << 4;

/// The scripts this DLL can name, by the number DirectWrite gives them.
/// Measured, one per script seen: there is no public list to read the
/// rest from, and a number guessed at would be worse than a join.
const LATIN: u16 = 49;
const ARABIC: u16 = 3;
const HAN: u16 = 33;

/// What a character contributes to script itemization.
enum Script {
    /// Carries no script of its own: it extends the run it falls in.
    Joins,
    Of(u16),
}

/// The script of `cp`, as itemization sees it.
fn script_of(cp: u32) -> Script {
    match cp {
        // Space, digits, ASCII punctuation and the Latin-1 symbols:
        // COMMON, which joins whatever run it falls in.
        0..=0x40 | 0x5B..=0x60 | 0x7B..=0xBF => Script::Joins,
        0x41..=0x5A | 0x61..=0x7A | 0xC0..=0x24F | 0x1E00..=0x1EFF => Script::Of(LATIN),
        0x600..=0x6FF | 0x750..=0x77F | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => Script::Of(ARABIC),
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => Script::Of(HAN),
        _ => {
            missing(
                b"dwrite.dll!IDWriteTextAnalyzer::AnalyzeScript(unnamed script)",
                u64::from(cp),
            );
            Script::Joins
        }
    }
}

/// A space: breakable after, and flagged isWhitespace.
fn is_space(cp: u32) -> bool {
    matches!(cp, 0x20 | 0x09 | 0x1680 | 0x2000..=0x200A | 0x205F | 0x3000)
}

/// An ideograph: breakable on either side.
fn is_ideograph(cp: u32) -> bool {
    matches!(cp, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

/// A character Windows marks MUST_BREAK after. Not measured, so it
/// breaks like a space here and says so (a stopgap, in ROADMAP).
fn is_hard_break(cp: u32) -> bool {
    matches!(cp, 0x0A | 0x0B | 0x0C | 0x0D | 0x85 | 0x2028 | 0x2029)
}

/// Whether a line may break between `a` and the character `b` after it.
fn can_break(a: u32, b: u32) -> bool {
    if is_hard_break(a) {
        missing(
            b"dwrite.dll!IDWriteTextAnalyzer::AnalyzeLineBreakpoints(hard line break)",
            u64::from(a),
        );
        return true;
    }
    is_space(a) || is_ideograph(a) || is_ideograph(b)
}

/// Whether `cp` is written right to left, which AnalyzeBidi cannot
/// resolve levels for yet.
fn is_rtl_char(cp: u32) -> bool {
    matches!(cp, 0x590..=0x8FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

/// Slot `slot` of the vtable of one of the caller's COM objects.
///
/// # Safety
/// `this` must be a COM object whose interface declares at least
/// `slot + 1` methods, and `F` the signature it declares at `slot`.
unsafe fn method<F: Copy>(this: *const c_void, slot: usize) -> F {
    // SAFETY: a COM object holds its vtable's address first.
    unsafe {
        let vtbl = *this.cast::<*const F>();
        *vtbl.add(slot)
    }
}

/// IDWriteTextAnalysisSource::GetTextAtPosition (3).
type GetTextAt = extern "system" fn(*const c_void, u32, *mut *const u16, *mut u32) -> HResult;
/// IDWriteTextAnalysisSink::SetScriptAnalysis (3).
type SetScript = extern "system" fn(*const c_void, u32, u32, *const ScriptAnalysis) -> HResult;
/// IDWriteTextAnalysisSink::SetLineBreakpoints (4).
type SetBreaks = extern "system" fn(*const c_void, u32, u32, *const u8) -> HResult;
/// IDWriteTextAnalysisSink::SetBidiLevel (5).
type SetBidi = extern "system" fn(*const c_void, u32, u32, u8, u8) -> HResult;

/// The source's text from `position`, however many pieces it answers
/// in, one UTF-16 unit or one code point at a time. The source is asked
/// once per piece and nothing is copied, which is why a whole range
/// answered at once -- what every source here does -- costs exactly one
/// GetTextAtPosition.
struct Units {
    source: *const c_void,
    /// Where the next piece starts.
    next: u32,
    /// Units of the range still to read.
    left: u32,
    /// The piece being read, and how far into it.
    piece: *const u16,
    have: u32,
    at: u32,
    /// A unit read and pushed back: already counted, so the counters
    /// above do not move for it again.
    held: Option<u16>,
}

impl Units {
    fn new(source: *const c_void, position: u32, length: u32) -> Self {
        Units {
            source,
            next: position,
            left: length,
            piece: core::ptr::null(),
            have: 0,
            at: 0,
            held: None,
        }
    }

    /// The next UTF-16 unit of the range.
    fn unit(&mut self) -> Option<u16> {
        if let Some(u) = self.held.take() {
            return Some(u);
        }
        if self.left == 0 {
            return None;
        }
        if self.at == self.have {
            let mut text: *const u16 = core::ptr::null();
            let mut len = 0u32;
            // SAFETY: slot 3 of the caller's IDWriteTextAnalysisSource,
            // called with that object.
            let hr = unsafe {
                method::<GetTextAt>(self.source, 3)(self.source, self.next, &mut text, &mut len)
            };
            if hr != S_OK || text.is_null() || len == 0 {
                // The source stopped short of the range it was asked
                // about: what is left is unreadable, so the run ends
                // here rather than guessing at it.
                missing(
                    b"dwrite.dll!IDWriteTextAnalysisSource::GetTextAtPosition(nothing there)",
                    u64::from(self.next),
                );
                self.left = 0;
                return None;
            }
            self.piece = text;
            self.have = len.min(self.left);
            self.at = 0;
        }
        // SAFETY: inside the piece the source answered with.
        let u = unsafe { *self.piece.add(self.at as usize) };
        self.at += 1;
        self.next += 1;
        self.left -= 1;
        Some(u)
    }

    /// The next code point and how many units it took.
    fn code(&mut self) -> Option<(u32, u32)> {
        let u = self.unit()?;
        if !(0xD800..0xDC00).contains(&u) {
            return Some((u32::from(u), 1));
        }
        // A high surrogate: the low one that follows makes one code point
        // with it, and anything else is pushed back to be read as itself.
        if let Some(l) = self.unit() {
            if (0xDC00..0xE000).contains(&l) {
                let cp = 0x1_0000 + ((u32::from(u) - 0xD800) << 10) + (u32::from(l) - 0xDC00);
                return Some((cp, 2));
            }
            self.held = Some(l);
        }
        Some((u32::from(u), 1))
    }
}

/// The code point at `i` of `text`, and how many units it took.
fn code_at(text: &[u16], i: usize) -> (u32, usize) {
    let u = text[i];
    if (0xD800..0xDC00).contains(&u)
        && let Some(&l) = text.get(i + 1)
        && (0xDC00..0xE000).contains(&l)
    {
        let cp = 0x1_0000 + ((u32::from(u) - 0xD800) << 10) + (u32::from(l) - 0xDC00);
        return (cp, 2);
    }
    (u32::from(u), 1)
}

/// The breakpoint bytes on their way to the sink, a block at a time.
/// One sink call for a line's worth of text, which is what Windows
/// does; a range longer than the block takes several.
struct Breaks {
    sink: *const c_void,
    /// The position `buf[0]` describes.
    at: u32,
    n: usize,
    buf: [u8; 256],
}

impl Breaks {
    fn new(sink: *const c_void, position: u32) -> Self {
        Breaks { sink, at: position, n: 0, buf: [0; 256] }
    }

    fn put(&mut self, b: u8) -> HResult {
        if self.n == self.buf.len() {
            let hr = self.flush();
            if hr != S_OK {
                return hr;
            }
        }
        self.buf[self.n] = b;
        self.n += 1;
        S_OK
    }

    fn flush(&mut self) -> HResult {
        if self.n == 0 {
            return S_OK;
        }
        // SAFETY: slot 4 of the caller's IDWriteTextAnalysisSink,
        // called with that object and our own buffer of `n` bytes.
        let hr = unsafe {
            method::<SetBreaks>(self.sink, 4)(self.sink, self.at, self.n as u32, self.buf.as_ptr())
        };
        self.at += self.n as u32;
        self.n = 0;
        hr
    }

    /// One character's bytes: `before` is the condition on its left and
    /// `after` the one on its right. A surrogate pair gets a byte per
    /// unit and may not break between them.
    fn character(&mut self, cp: u32, units: u32, before: u8, after: u8) -> HResult {
        if units == 2 {
            let hr = self.put(before | (MAY_NOT_BREAK << 2));
            if hr != S_OK {
                return hr;
            }
            return self.put(MAY_NOT_BREAK | (after << 2));
        }
        let ws = if is_space(cp) { IS_WHITESPACE } else { 0 };
        self.put(before | (after << 2) | ws)
    }
}

/// The analyzer object: its vtable's address and nothing else.
#[repr(C)]
struct Analyzer(*const ());
// SAFETY: a pointer to a static vtable, never written.
unsafe impl Sync for Analyzer {}

static ANALYZER: Analyzer = Analyzer(vt(&ANALYZER_VTBL));

/// IDWriteTextAnalyzer, in the order the header declares it. Revisions
/// 1 and 2 add methods this DLL does not have, so the vtable stops at
/// the first interface's ten slots: a program that asked for a revision
/// and called past them would read past this array, which is why
/// QueryInterface answering those IIDs is only sound while the methods
/// a caller reaches are these (dwrite_test step 12).
static ANALYZER_VTBL: VTable<10> = VTable([
    analyzer_qi as *const (),
    crate::static_add_ref as *const (),
    crate::static_release as *const (),
    analyzer_AnalyzeScript as *const (),
    analyzer_AnalyzeBidi as *const (),
    analyzer_AnalyzeNumberSubstitution as *const (),
    analyzer_AnalyzeLineBreakpoints as *const (),
    analyzer_GetGlyphs as *const (),
    analyzer_GetGlyphPlacements as *const (),
    analyzer_GetGdiCompatibleGlyphPlacements as *const (),
]);

extern "system" fn analyzer_qi(
    this: *const c_void,
    riid: *const Iid,
    out: *mut *const c_void,
) -> HResult {
    query(this, &ANALYZER_IIDS, riid, out)
}

/// IDWriteFactory::CreateTextAnalyzer (21): the one static analyzer.
/// Windows makes a fresh object per call; nothing here is ever freed,
/// so one object is indistinguishable. Windows also faults when `out`
/// is NULL, writing through it unchecked; this answers E_POINTER.
#[allow(non_snake_case)]
pub extern "system" fn factory_CreateTextAnalyzer(
    _this: *const c_void,
    out: *mut *const c_void,
) -> HResult {
    let r = if out.is_null() {
        E_POINTER
    } else {
        // SAFETY: the caller's output pointer, checked above.
        unsafe { *out = (&raw const ANALYZER).cast() };
        S_OK
    };
    answered(b"IDWriteFactory::CreateTextAnalyzer", r);
    r
}

/// Whether an `Analyze*` call has what it needs. Windows faults on a
/// source that cannot answer the position it is asked about; these
/// refuse a missing source or sink instead (measured: E_INVALIDARG).
fn analyzable(source: *const c_void, sink: *const c_void) -> HResult {
    if source.is_null() || sink.is_null() {
        return E_INVALIDARG;
    }
    S_OK
}

/// IDWriteTextAnalyzer::AnalyzeScript (3).
#[allow(non_snake_case)]
extern "system" fn analyzer_AnalyzeScript(
    _this: *const c_void,
    source: *const c_void,
    position: u32,
    length: u32,
    sink: *const c_void,
) -> HResult {
    reached!(b"IDWriteTextAnalyzer::AnalyzeScript");
    let hr = analyzable(source, sink);
    if hr != S_OK {
        return hr;
    }
    let mut units = Units::new(source, position, length);
    let mut start = position;
    let mut len = 0u32;
    let mut script: Option<u16> = None;
    while let Some((cp, n)) = units.code() {
        match script_of(cp) {
            Script::Joins => len += n,
            Script::Of(s) => match script {
                Some(open) if open != s => {
                    let hr = run(sink, start, len, open);
                    if hr != S_OK {
                        return hr;
                    }
                    start += len;
                    len = n;
                    script = Some(s);
                }
                _ => {
                    script = Some(s);
                    len += n;
                }
            },
        }
    }
    if len == 0 {
        return S_OK;
    }
    // A run of nothing but joining characters: COMMON takes the
    // paragraph's script, and this collection is Latin.
    run(sink, start, len, script.unwrap_or(LATIN))
}

/// One script run to the sink.
fn run(sink: *const c_void, position: u32, length: u32, script: u16) -> HResult {
    let a = ScriptAnalysis { script, shapes: SHAPES_DEFAULT };
    // SAFETY: slot 3 of the caller's IDWriteTextAnalysisSink, called
    // with that object and a struct that outlives the call.
    unsafe { method::<SetScript>(sink, 3)(sink, position, length, &a) }
}

/// IDWriteTextAnalyzer::AnalyzeBidi (4). One run at level 0: resolving
/// levels is a gap, named in the ledger when the text needs it.
#[allow(non_snake_case)]
extern "system" fn analyzer_AnalyzeBidi(
    _this: *const c_void,
    source: *const c_void,
    position: u32,
    length: u32,
    sink: *const c_void,
) -> HResult {
    reached!(b"IDWriteTextAnalyzer::AnalyzeBidi");
    let hr = analyzable(source, sink);
    if hr != S_OK {
        return hr;
    }
    if length == 0 {
        return S_OK;
    }
    let mut units = Units::new(source, position, length);
    while let Some((cp, _)) = units.code() {
        if is_rtl_char(cp) {
            missing(
                b"dwrite.dll!IDWriteTextAnalyzer::AnalyzeBidi(right-to-left text)",
                u64::from(cp),
            );
            break;
        }
    }
    // SAFETY: slot 5 of the caller's IDWriteTextAnalysisSink.
    unsafe { method::<SetBidi>(sink, 5)(sink, position, length, 0, 0) }
}

/// IDWriteTextAnalyzer::AnalyzeNumberSubstitution (5). S_OK with the
/// sink never called, over digits as well: measured, and a silent
/// success is the contract rather than a gap.
#[allow(non_snake_case)]
extern "system" fn analyzer_AnalyzeNumberSubstitution(
    _this: *const c_void,
    source: *const c_void,
    _position: u32,
    _length: u32,
    sink: *const c_void,
) -> HResult {
    reached!(b"IDWriteTextAnalyzer::AnalyzeNumberSubstitution");
    analyzable(source, sink)
}

/// IDWriteTextAnalyzer::AnalyzeLineBreakpoints (6).
#[allow(non_snake_case)]
extern "system" fn analyzer_AnalyzeLineBreakpoints(
    _this: *const c_void,
    source: *const c_void,
    position: u32,
    length: u32,
    sink: *const c_void,
) -> HResult {
    reached!(b"IDWriteTextAnalyzer::AnalyzeLineBreakpoints");
    let hr = analyzable(source, sink);
    if hr != S_OK {
        return hr;
    }
    let mut units = Units::new(source, position, length);
    let mut breaks = Breaks::new(sink, position);
    // One character of lookahead: a character's condition on the right
    // is the condition on the left of the one after it.
    let mut held: Option<(u32, u32, u8)> = None;
    while let Some((cp, n)) = units.code() {
        match held {
            Some((p, pn, before)) => {
                let gap = if can_break(p, cp) { CAN_BREAK } else { MAY_NOT_BREAK };
                let hr = breaks.character(p, pn, before, gap);
                if hr != S_OK {
                    return hr;
                }
                held = Some((cp, n, gap));
            }
            // The range's first character: no break before it.
            None => held = Some((cp, n, MAY_NOT_BREAK)),
        }
    }
    if let Some((p, pn, before)) = held {
        // The last character: a line may break after the range.
        let hr = breaks.character(p, pn, before, CAN_BREAK);
        if hr != S_OK {
            return hr;
        }
    }
    breaks.flush()
}

/// The face a shaping call names, as a parsed font.
fn shaping_face(font_face: *const c_void) -> Option<ttf_parser::Face<'static>> {
    let i = crate::font::font_of_face(font_face)?;
    crate::font::face(i)
}

/// IDWriteTextAnalyzer::GetGlyphs (7): one glyph per code point, the
/// glyph the face itself gives it. No feature, substitution, ligature
/// or mark attachment is applied, which is why every cluster is one
/// character long.
#[allow(non_snake_case)]
#[allow(clippy::too_many_arguments)]
extern "system" fn analyzer_GetGlyphs(
    _this: *const c_void,
    text: *const u16,
    text_len: u32,
    font_face: *const c_void,
    is_sideways: i32,
    is_rtl: i32,
    analysis: *const ScriptAnalysis,
    _locale: *const u16,
    _substitution: *const c_void,
    _features: *const *const c_void,
    _feature_range_lengths: *const u32,
    _feature_ranges: u32,
    max_glyph_count: u32,
    cluster_map: *mut u16,
    text_props: *mut u16,
    glyph_indices: *mut u16,
    glyph_props: *mut u16,
    glyph_count: *mut u32,
) -> HResult {
    reached!(b"IDWriteTextAnalyzer::GetGlyphs");
    if glyph_count.is_null() {
        return E_POINTER;
    }
    if text.is_null()
        || analysis.is_null()
        || cluster_map.is_null()
        || text_props.is_null()
        || glyph_indices.is_null()
        || glyph_props.is_null()
    {
        return E_INVALIDARG;
    }
    if is_sideways != 0 {
        missing(b"dwrite.dll!IDWriteTextAnalyzer::GetGlyphs(sideways run)", 0);
    }
    if is_rtl != 0 {
        missing(b"dwrite.dll!IDWriteTextAnalyzer::GetGlyphs(right-to-left run)", 0);
    }
    let Some(f) = shaping_face(font_face) else {
        missing(b"dwrite.dll!IDWriteTextAnalyzer::GetGlyphs(face not ours)", 0);
        return E_INVALIDARG;
    };
    // SAFETY: the caller's text of `text_len` units.
    let text = unsafe { core::slice::from_raw_parts(text, text_len as usize) };
    let mut g = 0usize;
    let mut i = 0usize;
    while i < text.len() {
        if g == max_glyph_count as usize {
            return E_INSUFFICIENT_BUFFER;
        }
        let (cp, n) = code_at(text, i);
        let id = char::from_u32(cp).and_then(|c| f.glyph_index(c)).map_or(0, |id| id.0);
        let justify = if cp == 0x20 { 4u16 } else { 2 };
        // SAFETY: the caller's output arrays, `g` inside
        // `max_glyph_count` and `i + n` inside the text's own length.
        unsafe {
            *glyph_indices.add(g) = id;
            *glyph_props.add(g) = IS_CLUSTER_START | justify;
            for k in 0..n {
                *cluster_map.add(i + k) = g as u16;
                // isShapedAlone and canBreakShapingAfter, neither
                // measured and neither read by the callers here.
                *text_props.add(i + k) = 0;
            }
        }
        g += 1;
        i += n;
    }
    // SAFETY: the caller's output, checked above.
    unsafe { *glyph_count = g as u32 };
    S_OK
}

/// IDWriteTextAnalyzer::GetGlyphPlacements (8): each glyph's own design
/// advance scaled by the em size, and no offsets. Measured to the last
/// bit against Windows on Segoe UI.
#[allow(non_snake_case)]
#[allow(clippy::too_many_arguments)]
extern "system" fn analyzer_GetGlyphPlacements(
    _this: *const c_void,
    _text: *const u16,
    _cluster_map: *const u16,
    _text_props: *const u16,
    _text_len: u32,
    glyph_indices: *const u16,
    glyph_props: *const u16,
    glyph_count: u32,
    font_face: *const c_void,
    em_size: f32,
    is_sideways: i32,
    _is_rtl: i32,
    analysis: *const ScriptAnalysis,
    _locale: *const u16,
    _features: *const *const c_void,
    _feature_range_lengths: *const u32,
    _feature_ranges: u32,
    advances: *mut f32,
    offsets: *mut GlyphOffset,
) -> HResult {
    reached!(b"IDWriteTextAnalyzer::GetGlyphPlacements");
    if glyph_indices.is_null() || glyph_props.is_null() || analysis.is_null() {
        return E_INVALIDARG;
    }
    if advances.is_null() || offsets.is_null() {
        return E_POINTER;
    }
    if is_sideways != 0 {
        missing(b"dwrite.dll!IDWriteTextAnalyzer::GetGlyphPlacements(sideways run)", 0);
    }
    let Some(f) = shaping_face(font_face) else {
        missing(b"dwrite.dll!IDWriteTextAnalyzer::GetGlyphPlacements(face not ours)", 0);
        return E_INVALIDARG;
    };
    let upem = f32::from(f.units_per_em());
    if upem == 0.0 {
        return E_FAIL;
    }
    for k in 0..glyph_count as usize {
        // SAFETY: the caller's arrays of `glyph_count` entries.
        unsafe {
            let id = ttf_parser::GlyphId(*glyph_indices.add(k));
            let design = f32::from(f.glyph_hor_advance(id).unwrap_or(0));
            *advances.add(k) = design * em_size / upem;
            *offsets.add(k) = GlyphOffset { advance: 0.0, ascender: 0.0 };
        }
    }
    S_OK
}

/// IDWriteTextAnalyzer::GetGdiCompatibleGlyphPlacements (9). Refused:
/// the GDI-compatible advances are rounded to whole pixels at a given
/// pixels-per-dip, which was not measured, and the non-GDI numbers
/// would be subtly wrong rather than honestly absent. Takes one
/// argument where the interface declares twenty-one, as the other
/// refusals in this DLL do, so nothing of the caller's is written.
#[allow(non_snake_case)]
extern "system" fn analyzer_GetGdiCompatibleGlyphPlacements(_this: *const c_void) -> HResult {
    missing(b"dwrite.dll!IDWriteTextAnalyzer::GetGdiCompatibleGlyphPlacements", 0);
    E_NOTIMPL
}
