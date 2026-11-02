//! A control's state (`Editor`) and its view: the text laid out in lines
//! (paragraphs wrapped at spaces to the format rectangle's width unless
//! WS_HSCROLL or EM_SETTARGETDEVICE turn wrapping off), each line's
//! character positions measured with gdi32 in each run's font, the
//! vertical scroll position in pixels (with the window's SB_VERT
//! information kept up to date), and painting: the background colour,
//! each run in its colour and font, the selection, the caret, links
//! underlined, and a thin scroll bar of gala's own drawn at the right
//! (user32 draws no scroll bars yet).

use crate::doc::{self, CFE_AUTOBACKCOLOR, CFE_AUTOCOLOR, CFE_LINK, CR, Doc, Format};
use crate::*;
use alloc::vec::Vec;

pub const WS_VSCROLL: u32 = 0x0020_0000;
pub const WS_HSCROLL: u32 = 0x0010_0000;
pub const WS_BORDER: u32 = 0x0080_0000;
pub const WS_DISABLED: u32 = 0x0800_0000;
pub const WS_EX_CLIENTEDGE: u32 = 0x200;
pub const ES_MULTILINE: u32 = 0x4;
pub const ES_NOHIDESEL: u32 = 0x100;
pub const ES_READONLY: u32 = 0x800;
pub const ES_DISABLENOSCROLL: u32 = 0x2000;
pub const GWL_STYLE: i32 = -16;
pub const GWL_EXSTYLE: i32 = -20;
/// The width of gala's scroll bar column.
pub const BAR: i32 = 12;
const COLOR_WINDOW: i32 = 5;
const COLOR_WINDOWTEXT: i32 = 8;
const COLOR_GRAYTEXT: i32 = 17;
const COLOR_HOTLIGHT: i32 = 26;
const SELECTION: u32 = 0x00d4_7800; // RGB(0x00, 0x78, 0xd4)
const SELECTION_INACTIVE: u32 = 0x00e0_e0e0;
const WHITE: u32 = 0x00ff_ffff;
const BORDER: u32 = 0x00e0_e0e0;
const THUMB: u32 = 0x00c2_c2c2;

/// One laid-out line: characters `start..end`, then a paragraph mark when
/// `brk`; its top, height and baseline (from the top); each character
/// boundary's x from the line's left (end - start + 1 of them).
pub struct Line {
    pub start: usize,
    pub end: usize,
    pub brk: bool,
    pub y: i32,
    pub h: i32,
    pub ascent: i32,
    /// Where the line starts right of the format rectangle's left edge
    /// (the paragraph's indent and alignment).
    pub indent: i32,
    pub xs: Vec<i32>,
}

/// The paragraph format (EM_SETPARAFORMAT), one for the whole control:
/// indents in twips (the first line's, the right one, and the further
/// lines' offset from the first), the alignment, tab stops in twips.
#[derive(Clone, Copy)]
pub struct Para {
    pub start: i32,
    pub right: i32,
    pub offset: i32,
    pub align: u16,
    pub tab_count: usize,
    pub tabs: [i32; 32],
}

pub const DEFAULT_PARA: Para =
    Para { start: 0, right: 0, offset: 0, align: 1, tab_count: 0, tabs: [0; 32] };

#[derive(Clone, Copy, PartialEq)]
struct FontKey {
    face: [u16; 32],
    px: i32,
    weight: u16,
    italic: bool,
    charset: u8,
}

struct FontEntry {
    key: FontKey,
    font: usize,
    height: i32,
    ascent: i32,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Drag {
    None,
    /// Selecting with the mouse.
    Select,
    /// The scroll bar's thumb, grabbed this many pixels below its top.
    Thumb(i32),
}

pub struct Editor {
    pub doc: Doc,
    /// An ANSI window ("RichEdit20A"): text in and out as ANSI.
    pub ansi: bool,
    pub anchor: usize,
    pub caret: usize,
    /// The format the next inserted text gets (EM_SETCHARFORMAT
    /// SCF_SELECTION on an empty selection), until the selection moves.
    pub insert: Option<u16>,
    pub mask: u32,
    /// The selection the parent last heard of (EN_SELCHANGE).
    pub notified: (usize, usize),
    pub limit: usize,
    /// EM_SETBKGNDCOLOR's colour (None: the system's window colour).
    pub bg: Option<u32>,
    pub mode: u32,
    pub auto_url: u32,
    pub links: Vec<(usize, usize)>,
    pub modified: bool,
    /// WM_SETFONT's font.
    pub font: usize,
    pub lang: u32,
    pub edit_style: u32,
    pub word_select: bool,
    pub hide_selection: bool,
    /// EM_SETTARGETDEVICE with a line width of 1: no wrapping.
    pub no_wrap: bool,
    /// EM_SETRECT's rectangle.
    pub rect: Option<[i32; 4]>,
    pub margins: (i32, i32),
    pub undo_limit: usize,
    pub para: Para,
    /// The vertical and horizontal scroll positions, in pixels.
    pub top: i32,
    pub left: i32,
    pub lines: Vec<Line>,
    pub total: i32,
    pub widest: i32,
    pub dirty: bool,
    fonts: Vec<FontEntry>,
    pub drag: Drag,
}

impl Editor {
    pub fn new(ansi: bool) -> Editor {
        // The default format: the GUI font's face and size.
        let mut lf = [0u8; 92];
        // SAFETY: a LOGFONTW-sized buffer; DEFAULT_GUI_FONT.
        let got = unsafe { GetObjectW(GetStockObject(17), 92, lf.as_mut_ptr()) };
        let (face, height) = if got >= 92 {
            // SAFETY: a LOGFONTW read from our buffer.
            let lf = unsafe { (lf.as_ptr() as *const LogFont).read_unaligned() };
            let n = lf.face.iter().position(|&c| c == 0).unwrap_or(32);
            let mut face = [0u16; 32];
            face[..n].copy_from_slice(&lf.face[..n]);
            (face, px_to_twips(lf.height))
        } else {
            let mut face = [0u16; 32];
            for (d, b) in face.iter_mut().zip(b"Segoe UI") {
                *d = u16::from(*b);
            }
            (face, 180)
        };
        let n = face.iter().position(|&c| c == 0).unwrap_or(32);
        Editor {
            doc: Doc::new(Format::new(&face[..n], height)),
            ansi,
            anchor: 0,
            caret: 0,
            insert: None,
            mask: 0,
            notified: (0, 0),
            limit: 32767,
            bg: None,
            // TM_RICHTEXT | TM_MULTILEVELUNDO | TM_MULTICODEPAGE
            mode: 2 | 8 | 32,
            auto_url: 0,
            links: Vec::new(),
            modified: false,
            font: 0,
            lang: 0,
            edit_style: 0,
            word_select: false,
            hide_selection: false,
            no_wrap: false,
            rect: None,
            margins: (0, 0),
            undo_limit: 100,
            para: DEFAULT_PARA,
            top: 0,
            left: 0,
            lines: Vec::new(),
            total: 0,
            widest: 0,
            dirty: true,
            fonts: Vec::new(),
            drag: Drag::None,
        }
    }

    pub fn selection(&self) -> (usize, usize) {
        (self.anchor.min(self.caret), self.anchor.max(self.caret))
    }

    /// The format text inserted at the selection gets.
    pub fn insert_format(&self) -> u16 {
        if let Some(f) = self.insert {
            return f;
        }
        let (a, _) = self.selection();
        if self.doc.len() == 0 {
            0
        } else if a > 0 {
            self.doc.format_at(a - 1)
        } else {
            self.doc.format_at(0)
        }
    }

    /// Whether `pos` is in a detected URL; its range.
    pub fn link_at(&self, pos: usize) -> Option<(usize, usize)> {
        if let Some(&l) = self.links.iter().find(|l| l.0 <= pos && pos < l.1) {
            return Some(l);
        }
        // Text formatted CFE_LINK by the program: its run's extent.
        if pos < self.doc.len() && self.format(self.doc.format_at(pos)).effects & CFE_LINK != 0 {
            let linked = |p: usize| self.format(self.doc.format_at(p)).effects & CFE_LINK != 0;
            let mut a = pos;
            while a > 0 && linked(a - 1) {
                a -= 1;
            }
            let mut b = pos;
            while b < self.doc.len() && linked(b) {
                b += 1;
            }
            return Some((a, b));
        }
        None
    }

    pub fn format(&self, i: u16) -> &Format {
        &self.doc.formats[usize::from(i)]
    }

    /// The text changed: links found again, the layout redone later.
    pub fn changed(&mut self) {
        self.links = if self.auto_url != 0 { doc::find_urls(&self.doc.text) } else { Vec::new() };
        self.dirty = true;
    }

    pub fn drop_fonts(&mut self) {
        for f in self.fonts.drain(..) {
            // SAFETY: our font.
            unsafe { DeleteObject(f.font) };
        }
    }

    /// The font for `f` (made and measured on `hdc` the first time): the
    /// font, its height and ascent.
    fn font_for(&mut self, hdc: usize, f: &Format) -> (usize, i32, i32) {
        let key = FontKey {
            face: f.face,
            px: twips_to_px(f.height),
            weight: f.weight,
            italic: f.effects & 0x2 != 0,
            charset: f.charset,
        };
        if let Some(e) = self.fonts.iter().find(|e| e.key == key) {
            return (e.font, e.height, e.ascent);
        }
        let lf = LogFont {
            height: -key.px,
            width: 0,
            escapement: 0,
            orientation: 0,
            weight: i32::from(key.weight),
            italic: u8::from(key.italic),
            underline: 0,
            strike_out: 0,
            char_set: key.charset,
            out_precision: 0,
            clip_precision: 0,
            quality: 0,
            pitch_and_family: f.pitch,
            face: key.face,
        };
        // SAFETY: our LOGFONTW; the DC's font is put back.
        let (font, height, ascent) = unsafe {
            let font = CreateFontIndirectW(&lf);
            let old = SelectObject(hdc, font);
            let mut tm = [0i32; 15];
            GetTextMetricsW(hdc, &mut tm);
            SelectObject(hdc, old);
            (font, tm[0].max(1), tm[1].max(1))
        };
        if self.fonts.len() >= 16 {
            let old = self.fonts.remove(0);
            // SAFETY: our font, no longer used.
            unsafe { DeleteObject(old.font) };
        }
        if self.fonts.try_reserve(1).is_ok() {
            self.fonts.push(FontEntry { key, font, height, ascent });
        }
        (font, height, ascent)
    }

    /// The line holding `pos` (the last one starting at or before it).
    pub fn line_of(&self, pos: usize) -> usize {
        let n = self.lines.partition_point(|l| l.start <= pos);
        n.saturating_sub(1)
    }

    /// `pos`'s x from the format rectangle's left edge (unscrolled).
    pub fn x_of(&self, line: usize, pos: usize) -> i32 {
        let l = &self.lines[line];
        let i = pos.clamp(l.start, l.end) - l.start;
        l.indent + l.xs.get(i).copied().unwrap_or(0)
    }

    /// The boundary nearest `x` (from the format rectangle's left edge,
    /// unscrolled) on `line`.
    pub fn pos_in_line(&self, line: usize, x: i32) -> usize {
        let l = &self.lines[line];
        let x = x - l.indent;
        for i in 0..l.end - l.start {
            if x < (l.xs[i] + l.xs[i + 1]) / 2 {
                return l.start + i;
            }
        }
        l.end
    }

    /// The line at `y` (document pixels), clamped.
    pub fn line_at_y(&self, y: i32) -> usize {
        let n = self.lines.partition_point(|l| l.y + l.h <= y);
        n.min(self.lines.len().saturating_sub(1))
    }

    /// The first line visible from the top.
    pub fn first_visible(&self) -> usize {
        self.line_at_y(self.top)
    }
}

pub fn twips_to_px(twips: i32) -> i32 {
    ((twips * 96 + 720) / 1440).max(1)
}

pub fn px_to_twips(height: i32) -> i32 {
    height.abs() * 1440 / 96
}

pub fn style(hwnd: Hwnd) -> u32 {
    // SAFETY: plain call.
    unsafe { GetWindowLongW(hwnd, GWL_STYLE) as u32 }
}

fn bordered(hwnd: Hwnd) -> bool {
    // SAFETY: plain call.
    let ex = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
    ex & WS_EX_CLIENTEDGE != 0 || style(hwnd) & WS_BORDER != 0
}

pub fn client(hwnd: Hwnd) -> [i32; 4] {
    let mut r = [0i32; 4];
    // SAFETY: our RECT.
    unsafe { GetClientRect(hwnd, &mut r) };
    r
}

/// The format rectangle (client coordinates): where text goes.
pub fn view(hwnd: Hwnd, e: &Editor) -> [i32; 4] {
    if let Some(r) = e.rect {
        return r;
    }
    let c = client(hwnd);
    let b = i32::from(bordered(hwnd));
    let bar = if style(hwnd) & WS_VSCROLL != 0 { BAR } else { 0 };
    [c[0] + b + 2 + e.margins.0, c[1] + b + 1, c[2] - b - bar - 1 - e.margins.1, c[3] - b]
}

pub fn wraps(hwnd: Hwnd, e: &Editor) -> bool {
    let s = style(hwnd);
    s & ES_MULTILINE != 0 && s & WS_HSCROLL == 0 && !e.no_wrap
}

/// Measures `s` in `font` on `hdc`: each prefix's width pushed onto `xs`
/// after `base`. A tab goes to the next of `tabs` (pixels from the format
/// rectangle's left, the paragraph starting at `origin`) or else to the
/// next half inch.
fn measure(
    hdc: usize,
    font: usize,
    s: &[u16],
    base: i32,
    xs: &mut Vec<i32>,
    origin: i32,
    tabs: &[i32],
) {
    // SAFETY: plain calls; the font is put back.
    let old = unsafe { SelectObject(hdc, font) };
    let mut x = base;
    let mut dx = [0i32; 256];
    for chunk in s.chunks(256) {
        let mut size = [0i32; 2];
        // SAFETY: `chunk.len()` characters and as many widths.
        unsafe {
            GetTextExtentExPointW(
                hdc,
                chunk.as_ptr(),
                chunk.len() as i32,
                i32::MAX,
                core::ptr::null_mut(),
                dx.as_mut_ptr(),
                &mut size,
            )
        };
        for (i, &c) in chunk.iter().enumerate() {
            let w = if c == 0x09 {
                let at = origin + x;
                let next = tabs.iter().copied().find(|&t| t > at).unwrap_or((at / 48 + 1) * 48);
                next - at
            } else {
                dx[i] - if i == 0 { 0 } else { dx[i - 1] }
            };
            x += w;
            let _ = xs.try_reserve(1);
            xs.push(x);
        }
    }
    // SAFETY: as above.
    unsafe { SelectObject(hdc, old) };
}

/// Lays the text out in lines when it changed (or the width did).
pub fn layout(hwnd: Hwnd, e: &mut Editor) {
    if !e.dirty {
        return;
    }
    e.dirty = false;
    let v = view(hwnd, e);
    let wrap = wraps(hwnd, e);
    let px = |tw: i32| tw * 96 / 1440;
    let para = e.para;
    let first_indent = px(para.start).max(0);
    let next_indent = px(para.start + para.offset).max(0);
    let avail = |indent: i32| (v[2] - v[0] - indent - px(para.right)).max(1);
    let mut tab_px = [0i32; 32];
    let tab_count = para.tab_count.min(32);
    for (t, &tw) in tab_px.iter_mut().zip(&para.tabs[..tab_count]) {
        *t = px(tw & 0x00ff_ffff);
    }
    let tabs = &tab_px[..tab_count];
    // SAFETY: plain call; released below.
    let hdc = unsafe { GetDC(hwnd) };
    let mut lines = Vec::new();
    let len = e.doc.len();
    let (mut y, mut widest) = (0, 0);
    let mut ps = 0;
    loop {
        let pe = e.doc.text[ps..].iter().position(|&c| c == CR).map_or(len, |i| ps + i);
        // Each boundary's x from the paragraph's start.
        let mut xs = Vec::new();
        let _ = xs.try_reserve(pe - ps + 1);
        xs.push(0);
        let pieces: Vec<(usize, usize, u16)> = e.doc.pieces(ps, pe).collect();
        let mut metrics = Vec::new();
        for &(a, b, f) in &pieces {
            let format = *e.format(f);
            let (font, h, asc) = e.font_for(hdc, &format);
            let base = *xs.last().unwrap_or(&0);
            measure(hdc, font, &e.doc.text[a..b], base, &mut xs, first_indent, tabs);
            if metrics.try_reserve(1).is_ok() {
                metrics.push((a, b, h, asc));
            }
        }
        // The empty paragraph's height: its mark's format.
        let mark = if pe < len { e.doc.format_at(pe) } else { e.insert_format_at_end() };
        let mark_format = *e.format(mark);
        let (_, mark_h, mark_asc) = e.font_for(hdc, &mark_format);
        let text = &e.doc.text;
        let mut s = ps;
        loop {
            let mut end = pe;
            let indent = if s == ps { first_indent } else { next_indent };
            let width = if wrap { avail(indent) } else { i32::MAX };
            if s < pe {
                let fits = |k: usize| xs[k - ps] - xs[s - ps] <= width;
                let mut k = s + 1;
                while k < pe && fits(k + 1) {
                    k += 1;
                }
                end = k;
                if end < pe {
                    // Spaces hang at the end; otherwise break after the
                    // last space, if any.
                    if text[end] == 0x20 {
                        while end < pe && text[end] == 0x20 {
                            end += 1;
                        }
                    } else if let Some(j) =
                        text[s..end].iter().rposition(|&c| c == 0x20 || c == 0x09)
                    {
                        end = s + j + 1;
                    }
                }
            }
            let (mut h, mut asc) = (0, 0);
            for &(a, b, mh, ma) in &metrics {
                if a < end.max(s + 1) && b > s {
                    h = h.max(mh);
                    asc = asc.max(ma);
                }
            }
            if h == 0 {
                h = mark_h;
                asc = mark_asc;
            }
            let mut lx = Vec::new();
            if lx.try_reserve(end - s + 1).is_ok() {
                lx.extend(xs[s - ps..=end - ps].iter().map(|&x| x - xs[s - ps]));
            }
            let line_w = *lx.last().unwrap_or(&0);
            // PFA_RIGHT (2), PFA_CENTER (3).
            let room = if wrap { avail(indent) } else { v[2] - v[0] - indent };
            let shift = match para.align {
                2 => (room - line_w).max(0),
                3 => (room - line_w).max(0) / 2,
                _ => 0,
            };
            widest = widest.max(indent + line_w);
            let brk = end == pe && pe < len;
            if lines.try_reserve(1).is_ok() {
                let indent = indent + shift;
                lines.push(Line { start: s, end, brk, y, h, ascent: asc, indent, xs: lx });
            }
            y += h;
            s = end;
            if s >= pe {
                break;
            }
        }
        if pe >= len {
            break;
        }
        ps = pe + 1;
    }
    // SAFETY: the DC from GetDC.
    unsafe { ReleaseDC(hwnd, hdc) };
    e.lines = lines;
    e.total = y;
    e.widest = widest;
    let v_h = v[3] - v[1];
    e.top = e.top.clamp(0, (e.total - v_h).max(0));
}

impl Editor {
    /// The format of an empty last paragraph: what typing there gets.
    fn insert_format_at_end(&self) -> u16 {
        if self.doc.len() == 0 {
            self.insert.unwrap_or(0)
        } else {
            self.doc.format_at(self.doc.len() - 1)
        }
    }
}

/// The largest scroll position.
pub fn max_top(hwnd: Hwnd, e: &Editor) -> i32 {
    let v = view(hwnd, e);
    (e.total - (v[3] - v[1])).max(0)
}

/// Scrolls so that the caret's line shows (and, without wrapping, its x).
pub fn caret_into_view(hwnd: Hwnd, e: &mut Editor) {
    layout(hwnd, e);
    let v = view(hwnd, e);
    let line = e.line_of(e.caret);
    let (y, h) = (e.lines[line].y, e.lines[line].h);
    let vh = v[3] - v[1];
    if y < e.top {
        e.top = y;
    } else if y + h > e.top + vh {
        e.top = y + h - vh;
    }
    e.top = e.top.clamp(0, max_top(hwnd, e));
    if !wraps(hwnd, e) {
        let x = e.x_of(line, e.caret);
        let vw = (v[2] - v[0]).max(1);
        if x < e.left {
            e.left = x;
        } else if x > e.left + vw - 1 {
            e.left = x - vw + 1;
        }
    } else {
        e.left = 0;
    }
}

/// The window's SB_VERT information for its current layout.
pub fn scroll_info(hwnd: Hwnd, e: &Editor) -> ScrollInfo {
    let v = view(hwnd, e);
    ScrollInfo {
        size: core::mem::size_of::<ScrollInfo>() as u32,
        // SIF_RANGE | SIF_PAGE | SIF_POS (| SIF_DISABLENOSCROLL)
        mask: 0x7 | if style(hwnd) & ES_DISABLENOSCROLL != 0 { 0x8 } else { 0 },
        min: 0,
        max: (e.total - 1).max(0),
        page: (v[3] - v[1]).max(0) as u32,
        pos: e.top,
        track: 0,
    }
}

/// The scroll bar's thumb (top, height) in client coordinates, if the
/// text does not fit.
pub fn thumb(hwnd: Hwnd, e: &Editor) -> Option<(i32, i32)> {
    let c = client(hwnd);
    let b = i32::from(bordered(hwnd));
    let track = c[3] - c[1] - 2 * b - 4;
    let v = view(hwnd, e);
    let vh = v[3] - v[1];
    if e.total <= vh || track <= 0 || style(hwnd) & WS_VSCROLL == 0 {
        return None;
    }
    let h = (track * vh / e.total).max(16).min(track);
    let range = (e.total - vh).max(1);
    let y = c[1] + b + 2 + (track - h) * e.top / range;
    Some((y, h))
}

fn fill(hdc: usize, r: [i32; 4], color: u32) {
    if r[2] <= r[0] || r[3] <= r[1] {
        return;
    }
    // SAFETY: plain calls; the brush is deleted.
    unsafe {
        let brush = CreateSolidBrush(color);
        FillRect(hdc, &r, brush);
        DeleteObject(brush);
    }
}

fn sys(index: i32) -> u32 {
    // SAFETY: plain call.
    unsafe { GetSysColor(index) }
}

/// Paints the control on `hdc` (BeginPaint's, taken outside the state's
/// lock: it sends WM_ERASEBKGND).
pub fn paint(hwnd: Hwnd, e: &mut Editor, hdc: usize, focused: bool) {
    layout(hwnd, e);
    let s = style(hwnd);
    let c = client(hwnd);
    let v = view(hwnd, e);
    let disabled = s & WS_DISABLED != 0;
    let bg = e.bg.unwrap_or_else(|| sys(COLOR_WINDOW));
    fill(hdc, c, bg);
    if bordered(hwnd) {
        fill(hdc, [c[0], c[1], c[2], c[1] + 1], BORDER);
        fill(hdc, [c[0], c[3] - 1, c[2], c[3]], BORDER);
        fill(hdc, [c[0], c[1], c[0] + 1, c[3]], BORDER);
        fill(hdc, [c[2] - 1, c[1], c[2], c[3]], BORDER);
    }
    if let Some((y, h)) = thumb(hwnd, e) {
        let x = c[2] - i32::from(bordered(hwnd)) - BAR / 2 - 2;
        fill(hdc, [x, y, x + 4, y + h], THUMB);
    }
    // SAFETY: plain calls on the paint DC.
    unsafe {
        IntersectClipRect(hdc, v[0], v[1], v[2], v[3]);
        SetBkMode(hdc, 1);
    }
    let (sa, sb) = e.selection();
    let show_selection = sa != sb && !e.hide_selection && (focused || s & ES_NOHIDESEL != 0);
    let first = e.first_visible();
    for li in first..e.lines.len() {
        let (start, end, brk, ly, lh, lasc) = {
            let l = &e.lines[li];
            (l.start, l.end, l.brk, l.y, l.h, l.ascent)
        };
        let y = v[1] + ly - e.top;
        if y >= v[3] {
            break;
        }
        let x0 = v[0] - e.left;
        // Pieces split at runs, the selection and links.
        let mut cuts: Vec<usize> = Vec::new();
        let _ = cuts.try_reserve(8);
        for (a, _, _) in e.doc.pieces(start, end) {
            cuts.push(a);
        }
        for p in [sa, sb] {
            if p > start && p < end {
                cuts.push(p);
            }
        }
        for &(a, b) in &e.links {
            for p in [a, b] {
                if p > start && p < end && cuts.try_reserve(1).is_ok() {
                    cuts.push(p);
                }
            }
        }
        cuts.push(end);
        cuts.sort_unstable();
        cuts.dedup();
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if a >= b {
                continue;
            }
            let format = *e.format(e.doc.format_at(a));
            let (font, _, asc) = e.font_for(hdc, &format);
            let xa = x0 + e.x_of(li, a);
            let xb = x0 + e.x_of(li, b);
            let selected = show_selection && a >= sa && b <= sb;
            let link =
                e.links.iter().any(|l| l.0 <= a && b <= l.1) || format.effects & CFE_LINK != 0;
            if selected {
                fill(
                    hdc,
                    [xa, y, xb, y + lh],
                    if focused { SELECTION } else { SELECTION_INACTIVE },
                );
            } else if format.effects & CFE_AUTOBACKCOLOR == 0 {
                fill(hdc, [xa, y, xb, y + lh], format.back);
            }
            let color = if disabled {
                sys(COLOR_GRAYTEXT)
            } else if selected && focused {
                WHITE
            } else if link && format.effects & CFE_AUTOCOLOR != 0 {
                sys(COLOR_HOTLIGHT)
            } else if format.effects & CFE_AUTOCOLOR != 0 {
                sys(COLOR_WINDOWTEXT)
            } else {
                format.color
            };
            let base = y + lasc;
            // Superscript/subscript offsets (twips, up).
            let rise = twips_to_px(format.offset.abs()) * format.offset.signum();
            let text = &e.doc.text[a..b];
            // SAFETY: plain calls on the paint DC with our text.
            unsafe {
                let old = SelectObject(hdc, font);
                SetTextColor(hdc, color);
                let mut x = xa;
                let mut i = a;
                for chunk in text.split_inclusive(|&c| c == 0x09) {
                    let shown = chunk.strip_suffix(&[0x09]).unwrap_or(chunk);
                    for part in shown.chunks(256) {
                        TextOutW(hdc, x, base - asc - rise, part.as_ptr(), part.len() as i32);
                        i += part.len();
                        x = x0 + e.x_of(li, i);
                    }
                    if shown.len() < chunk.len() {
                        i += 1;
                        x = x0 + e.x_of(li, i);
                    }
                }
                SelectObject(hdc, old);
            }
            if format.effects & doc::CFE_UNDERLINE != 0 || link {
                fill(hdc, [xa, base + 1, xb, base + 2], color);
            }
            if format.effects & doc::CFE_STRIKEOUT != 0 {
                let m = base - asc / 3;
                fill(hdc, [xa, m, xb, m + 1], color);
            }
        }
        // A selected paragraph mark shows as a small block.
        if show_selection && brk && end >= sa && end < sb {
            let x = x0 + e.x_of(li, end);
            fill(hdc, [x, y, x + 4, y + lh], if focused { SELECTION } else { SELECTION_INACTIVE });
        }
    }
    // The caret.
    if focused && sa == sb && !disabled && !e.lines.is_empty() {
        let li = e.line_of(e.caret);
        let l = &e.lines[li];
        let x = v[0] - e.left + e.x_of(li, e.caret);
        let y = v[1] + l.y - e.top;
        fill(hdc, [x, y, x + 1, y + l.h], sys(COLOR_WINDOWTEXT));
    }
}
