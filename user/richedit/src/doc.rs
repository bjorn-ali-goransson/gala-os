//! The text model: UTF-16 text whose paragraphs end in a CR each (rich
//! edit's paragraph mark; the document's last paragraph has none), runs
//! of character formats over it, the formats themselves (CHARFORMAT2's
//! fields; index 0 of the table is the control's default format, which
//! text never formatted follows), and the conversions programs see:
//! CRLF on the way out of WM_GETTEXT, CRLF and LF to CR on the way in.

use alloc::vec::Vec;

pub const CR: u16 = 0x0d;
pub const LF: u16 = 0x0a;

// CHARFORMAT's dwMask bits (CFM_*) and dwEffects bits (CFE_*).
pub const CFM_BOLD: u32 = 0x1;
pub const CFM_ITALIC: u32 = 0x2;
pub const CFM_UNDERLINE: u32 = 0x4;
pub const CFM_STRIKEOUT: u32 = 0x8;
pub const CFM_PROTECTED: u32 = 0x10;
pub const CFM_LINK: u32 = 0x20;
pub const CFM_SIZE: u32 = 0x8000_0000;
pub const CFM_COLOR: u32 = 0x4000_0000;
pub const CFM_FACE: u32 = 0x2000_0000;
pub const CFM_OFFSET: u32 = 0x1000_0000;
pub const CFM_CHARSET: u32 = 0x0800_0000;
pub const CFM_BACKCOLOR: u32 = 0x0400_0000;
pub const CFM_LCID: u32 = 0x0200_0000;
pub const CFM_UNDERLINETYPE: u32 = 0x0080_0000;
pub const CFM_WEIGHT: u32 = 0x0040_0000;
pub const CFM_SPACING: u32 = 0x0020_0000;
pub const CFE_BOLD: u32 = 0x1;
pub const CFE_UNDERLINE: u32 = 0x4;
pub const CFE_STRIKEOUT: u32 = 0x8;
pub const CFE_LINK: u32 = 0x20;
pub const CFE_AUTOCOLOR: u32 = 0x4000_0000;
pub const CFE_AUTOBACKCOLOR: u32 = 0x0400_0000;
/// The effects CHARFORMAT (version 1) knows.
const CFM_EFFECTS: u32 =
    CFM_BOLD | CFM_ITALIC | CFM_UNDERLINE | CFM_STRIKEOUT | CFM_PROTECTED | CFM_LINK;
/// CHARFORMAT2's further on/off effects (small caps .. revised, sub- and
/// superscript): their mask bits are their effect bits.
const CFM_EFFECTS2_ONLY: u32 = 0x0000_7fc0 | 0x0003_0000;
pub const CFM_ALL: u32 = CFM_EFFECTS | CFM_COLOR | CFM_SIZE | CFM_FACE | CFM_OFFSET | CFM_CHARSET;
pub const CFM_ALL2: u32 = CFM_ALL
    | CFM_EFFECTS2_ONLY
    | CFM_BACKCOLOR
    | CFM_LCID
    | CFM_UNDERLINETYPE
    | CFM_WEIGHT
    | CFM_SPACING;

/// One character format (CHARFORMAT2's fields that gala keeps).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub effects: u32,
    /// In twips.
    pub height: i32,
    pub offset: i32,
    pub color: u32,
    pub back: u32,
    pub charset: u8,
    pub pitch: u8,
    pub weight: u16,
    pub spacing: i16,
    pub lcid: u32,
    pub underline_type: u8,
    pub face: [u16; 32],
}

impl Format {
    /// The default: automatic colours, the GUI font's face and size.
    pub fn new(face: &[u16], height: i32) -> Format {
        let mut f = Format {
            effects: CFE_AUTOCOLOR | CFE_AUTOBACKCOLOR,
            height,
            offset: 0,
            color: 0,
            back: 0,
            charset: 1,
            pitch: 0,
            weight: 400,
            spacing: 0,
            lcid: 0x0409,
            underline_type: 0,
            face: [0; 32],
        };
        for (d, &s) in f.face.iter_mut().zip(face.iter().take(31)) {
            *d = s;
        }
        f
    }

    pub fn face(&self) -> &[u16] {
        let n = self.face.iter().position(|&c| c == 0).unwrap_or(32);
        &self.face[..n]
    }
}

/// The sizes of CHARFORMATA, CHARFORMATW, CHARFORMAT2A, CHARFORMAT2W.
const SIZES: [(u32, bool, bool); 4] =
    [(60, false, true), (92, false, false), (84, true, true), (116, true, false)];

/// A CHARFORMAT(2)(A/W)'s kind from its cbSize: (version 2, ANSI face).
pub fn kind(cf: *const u8) -> Option<(bool, bool)> {
    if cf.is_null() {
        return None;
    }
    // SAFETY: the caller's structure starts with cbSize.
    let size = unsafe { (cf as *const u32).read_unaligned() };
    SIZES.iter().find(|s| s.0 == size).map(|s| (s.1, s.2))
}

fn rd32(p: *const u8, at: usize) -> u32 {
    // SAFETY: callers pass offsets inside the structure `kind` sized.
    unsafe { (p.add(at) as *const u32).read_unaligned() }
}

fn wr32(p: *mut u8, at: usize, v: u32) {
    // SAFETY: as `rd32`.
    unsafe { (p.add(at) as *mut u32).write_unaligned(v) }
}

/// The offset of the CHARFORMAT2 fields after the face name.
fn tail(ansi: bool) -> usize {
    if ansi { 58 } else { 90 }
}

/// What `cf` (a CHARFORMAT of a size `kind` accepted) sets: its mask
/// (CHARFORMAT's own bits only, for version 1) and `base` changed by it.
pub fn apply(cf: *const u8, base: &Format) -> Format {
    let Some((v2, ansi)) = kind(cf) else { return *base };
    let mask = rd32(cf, 4) & if v2 { CFM_ALL2 } else { CFM_ALL };
    let effects = rd32(cf, 8);
    let mut f = *base;
    let onoff = mask & (CFM_EFFECTS | CFM_EFFECTS2_ONLY);
    f.effects = (f.effects & !onoff) | (effects & onoff);
    if mask & CFM_BOLD != 0 {
        f.weight = if effects & CFE_BOLD != 0 { 700 } else { 400 };
    }
    if mask & CFM_COLOR != 0 {
        f.color = rd32(cf, 20);
        f.effects = (f.effects & !CFE_AUTOCOLOR) | (effects & CFE_AUTOCOLOR);
    }
    if mask & CFM_SIZE != 0 {
        f.height = rd32(cf, 12) as i32;
    }
    if mask & CFM_OFFSET != 0 {
        f.offset = rd32(cf, 16) as i32;
    }
    // SAFETY: bytes 24 and 25 are inside every CHARFORMAT.
    let (charset, pitch) = unsafe { (*cf.add(24), *cf.add(25)) };
    if mask & CFM_CHARSET != 0 {
        f.charset = charset;
    }
    if mask & CFM_FACE != 0 {
        f.pitch = pitch;
        f.face = [0; 32];
        for i in 0..31 {
            // SAFETY: the face name's 32 characters at offset 26.
            let c = unsafe {
                if ansi {
                    u16::from(*cf.add(26 + i))
                } else {
                    (cf.add(26 + 2 * i) as *const u16).read_unaligned()
                }
            };
            if c == 0 {
                break;
            }
            f.face[i] = c;
        }
    }
    if v2 {
        let t = tail(ansi);
        // SAFETY: the CHARFORMAT2 fields after the face name.
        let (weight, spacing) = unsafe {
            (
                (cf.add(t) as *const u16).read_unaligned(),
                (cf.add(t + 2) as *const i16).read_unaligned(),
            )
        };
        if mask & CFM_WEIGHT != 0 && weight != 0 {
            f.weight = weight;
            f.effects = if weight >= 700 { f.effects | CFE_BOLD } else { f.effects & !CFE_BOLD };
        }
        if mask & CFM_SPACING != 0 {
            f.spacing = spacing;
        }
        if mask & CFM_BACKCOLOR != 0 {
            f.back = rd32(cf, t + 6);
            f.effects = (f.effects & !CFE_AUTOBACKCOLOR) | (effects & CFE_AUTOBACKCOLOR);
        }
        if mask & CFM_LCID != 0 {
            f.lcid = rd32(cf, t + 10);
        }
        if mask & CFM_UNDERLINETYPE != 0 {
            // SAFETY: bUnderlineType inside CHARFORMAT2.
            f.underline_type = unsafe { *cf.add(t + 22) };
        }
    }
    f
}

/// Fills `cf` (a CHARFORMAT of a size `kind` accepted) with `f`, its mask
/// `mask` (cut to what the structure has); returns that mask.
pub fn fill(cf: *mut u8, f: &Format, mask: u32) -> u32 {
    let Some((v2, ansi)) = kind(cf) else { return 0 };
    let mask = mask & if v2 { CFM_ALL2 } else { CFM_ALL };
    let size = rd32(cf, 0) as usize;
    // SAFETY: the caller's structure of `size` bytes; cbSize kept.
    unsafe { core::ptr::write_bytes(cf.add(4), 0, size - 4) };
    wr32(cf, 4, mask);
    let effects = f.effects & if v2 { !0 } else { CFM_EFFECTS | CFE_AUTOCOLOR };
    wr32(cf, 8, effects);
    wr32(cf, 12, f.height as u32);
    wr32(cf, 16, f.offset as u32);
    wr32(cf, 20, f.color);
    // SAFETY: bytes 24 and 25, and the face name at 26 (32 characters).
    unsafe {
        *cf.add(24) = f.charset;
        *cf.add(25) = f.pitch;
        for (i, &c) in f.face().iter().enumerate().take(31) {
            if ansi {
                *cf.add(26 + i) = if c < 0x100 { c as u8 } else { b'?' };
            } else {
                (cf.add(26 + 2 * i) as *mut u16).write_unaligned(c);
            }
        }
    }
    if v2 {
        let t = tail(ansi);
        // SAFETY: the CHARFORMAT2 fields after the face name.
        unsafe {
            (cf.add(t) as *mut u16).write_unaligned(f.weight);
            (cf.add(t + 2) as *mut i16).write_unaligned(f.spacing);
            *cf.add(t + 22) = f.underline_type;
        }
        wr32(cf, t + 6, f.back);
        wr32(cf, t + 10, f.lcid);
    }
    mask
}

/// The mask bits on which `a` and `b` agree.
pub fn agree(a: &Format, b: &Format) -> u32 {
    let mut mask = CFM_ALL2;
    mask &= !((a.effects ^ b.effects) & (CFM_EFFECTS | CFM_EFFECTS2_ONLY));
    if a.color != b.color || (a.effects ^ b.effects) & CFE_AUTOCOLOR != 0 {
        mask &= !CFM_COLOR;
    }
    if a.back != b.back || (a.effects ^ b.effects) & CFE_AUTOBACKCOLOR != 0 {
        mask &= !CFM_BACKCOLOR;
    }
    let pairs = [
        (a.height != b.height, CFM_SIZE),
        (a.offset != b.offset, CFM_OFFSET),
        (a.charset != b.charset, CFM_CHARSET),
        (a.face != b.face || a.pitch != b.pitch, CFM_FACE),
        (a.weight != b.weight, CFM_WEIGHT),
        (a.spacing != b.spacing, CFM_SPACING),
        (a.lcid != b.lcid, CFM_LCID),
        (a.underline_type != b.underline_type, CFM_UNDERLINETYPE),
    ];
    for (differ, bit) in pairs {
        if differ {
            mask &= !bit;
        }
    }
    mask
}

/// A run of characters in one format (an index into the format table).
#[derive(Clone, Copy)]
pub struct Run {
    pub len: usize,
    pub format: u16,
}

pub struct Doc {
    pub text: Vec<u16>,
    pub runs: Vec<Run>,
    pub formats: Vec<Format>,
}

impl Doc {
    pub fn new(default: Format) -> Doc {
        let mut formats = Vec::new();
        let _ = formats.try_reserve(8);
        formats.push(default);
        Doc { text: Vec::new(), runs: Vec::new(), formats }
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    /// The table index of `f` (added if new; the default when full).
    pub fn intern(&mut self, f: Format) -> u16 {
        if let Some(i) = self.formats.iter().position(|g| *g == f) {
            return i as u16;
        }
        if self.formats.len() >= 0xffff || self.formats.try_reserve(1).is_err() {
            return 0;
        }
        self.formats.push(f);
        (self.formats.len() - 1) as u16
    }

    /// The format of the character at `pos` (the last one's past the end;
    /// the default in an empty text).
    pub fn format_at(&self, pos: usize) -> u16 {
        let mut start = 0;
        for r in &self.runs {
            if pos < start + r.len {
                return r.format;
            }
            start += r.len;
        }
        self.runs.last().map_or(0, |r| r.format)
    }

    /// Makes `pos` a run boundary; the index of the run starting there.
    fn split(&mut self, pos: usize) -> usize {
        let mut start = 0;
        for i in 0..self.runs.len() {
            let r = self.runs[i];
            if pos == start {
                return i;
            }
            if pos < start + r.len {
                if self.runs.try_reserve(1).is_err() {
                    return i;
                }
                self.runs[i].len = pos - start;
                self.runs.insert(i + 1, Run { len: start + r.len - pos, format: r.format });
                return i + 1;
            }
            start += r.len;
        }
        self.runs.len()
    }

    /// Joins neighbouring runs of one format and drops empty ones.
    fn merge(&mut self) {
        let mut out = 0;
        for i in 0..self.runs.len() {
            let r = self.runs[i];
            if r.len == 0 {
                continue;
            }
            if out > 0 && self.runs[out - 1].format == r.format {
                self.runs[out - 1].len += r.len;
            } else {
                self.runs[out] = r;
                out += 1;
            }
        }
        self.runs.truncate(out);
    }

    /// Changes the format of `a..b` with `f`.
    pub fn format_range(&mut self, a: usize, b: usize, f: impl Fn(&Format) -> Format) {
        let (a, b) = (a.min(self.len()), b.min(self.len()));
        if a >= b {
            return;
        }
        let first = self.split(a);
        let last = self.split(b);
        for i in first..last {
            let old = self.formats[usize::from(self.runs[i].format)];
            let new = f(&old);
            self.runs[i].format = self.intern(new);
        }
        self.merge();
    }

    /// Replaces `a..b` with `with` (CR paragraph marks) in `format`;
    /// false when out of memory (nothing changed).
    pub fn replace(&mut self, a: usize, b: usize, with: &[u16], format: u16) -> bool {
        let (a, b) = (a.min(self.len()), b.min(self.len()).max(a.min(self.len())));
        if self.text.try_reserve(with.len()).is_err() || self.runs.try_reserve(3).is_err() {
            return false;
        }
        let first = self.split(a);
        let last = self.split(b);
        self.runs.drain(first..last);
        self.text.splice(a..b, with.iter().copied());
        if !with.is_empty() {
            self.runs.insert(first, Run { len: with.len(), format });
        }
        self.merge();
        true
    }

    /// The characters of `a..b` (clamped).
    pub fn slice(&self, a: usize, b: usize) -> &[u16] {
        let b = b.min(self.len());
        &self.text[a.min(b)..b]
    }

    /// The runs over `a..b` as (start, end, format) pieces.
    pub fn pieces(&self, a: usize, b: usize) -> impl Iterator<Item = (usize, usize, u16)> + '_ {
        let mut start = 0;
        self.runs.iter().filter_map(move |r| {
            let (s, e) = (start, start + r.len);
            start = e;
            let (s, e) = (s.max(a), e.min(b));
            (s < e).then_some((s, e, r.format))
        })
    }
}

/// `s` with CRLF, a lone LF and a lone CR each made one CR; a single-line
/// control keeps only the first paragraph.
pub fn to_marks(s: &[u16], single_line: bool) -> Vec<u16> {
    let mut out = Vec::new();
    if out.try_reserve(s.len()).is_err() {
        return out;
    }
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if c == CR || c == LF {
            if single_line {
                break;
            }
            out.push(CR);
            if c == CR && s.get(i + 1) == Some(&LF) {
                i += 1;
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// The length of `s` with every CR as CRLF.
pub fn crlf_len(s: &[u16]) -> usize {
    s.len() + s.iter().filter(|&&c| c == CR).count()
}

/// Copies `s` (CRLF for CR when `crlf`) into `out` (room for `room`
/// characters and a NUL); the characters copied.
pub fn copy_out(s: &[u16], crlf: bool, out: *mut u16, room: usize) -> usize {
    if out.is_null() || room == 0 {
        return 0;
    }
    let mut n = 0;
    for &c in s {
        let pair: &[u16] = if crlf && c == CR { &[CR, LF] } else { core::slice::from_ref(&c) };
        if n + pair.len() > room - 1 {
            break;
        }
        for &p in pair {
            // SAFETY: `n` < room - 1 within the caller's buffer.
            unsafe { *out.add(n) = p };
            n += 1;
        }
    }
    // SAFETY: n < room.
    unsafe { *out.add(n) = 0 };
    n
}

/// Whether `c` ends a URL.
fn url_end(c: u16) -> bool {
    matches!(
        c,
        0x20 | 0x09 | CR | LF | 0x22 | 0x3c | 0x3e | 0x7b | 0x7d | 0x7c | 0x5c | 0x5e | 0x60
    ) || c < 0x20
}

/// The URLs in `text` that automatic URL detection finds (EM_AUTOURLDETECT):
/// a scheme Microsoft's documentation lists, or "www.", at the start of a
/// word, up to white space or a character URLs cannot hold; final
/// punctuation is left out.
pub fn find_urls(text: &[u16]) -> Vec<(usize, usize)> {
    const PREFIXES: [&[u8]; 14] = [
        b"http://",
        b"https://",
        b"ftp://",
        b"file:",
        b"mailto:",
        b"news:",
        b"nntp:",
        b"telnet:",
        b"gopher:",
        b"wais:",
        b"prospero:",
        b"notes:",
        b"onenote:",
        b"www.",
    ];
    let lower = |c: u16| if (0x41..=0x5a).contains(&c) { c + 32 } else { c };
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let word_start = i == 0 || url_end(text[i - 1]) || matches!(text[i - 1], 0x28 | 0x5b);
        let hit = word_start
            && PREFIXES.iter().any(|p| {
                text.len() - i > p.len()
                    && p.iter().enumerate().all(|(k, &b)| lower(text[i + k]) == u16::from(b))
            });
        if !hit {
            i += 1;
            continue;
        }
        let mut e = i;
        while e < text.len() && !url_end(text[e]) {
            e += 1;
        }
        while e > i && matches!(text[e - 1], 0x2e | 0x2c | 0x3b | 0x3a | 0x21 | 0x3f | 0x29 | 0x27)
        {
            e -= 1;
        }
        if out.try_reserve(1).is_ok() {
            out.push((i, e));
        }
        i = e.max(i + 1);
    }
    out
}
