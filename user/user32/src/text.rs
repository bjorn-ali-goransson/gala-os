//! DrawTextW (and DrawTextA, DrawTextExW) over gdi32's text functions:
//! lines split at line feeds (or one line with DT_SINGLELINE), wrapped at
//! spaces with DT_WORDBREAK, cut with "..." for DT_END_ELLIPSIS; '&'
//! prefixes removed ("&&" is '&') unless DT_NOPREFIX (no underline is
//! drawn); placed left, centred or right, and for one line also
//! vertically; DT_CALCRECT measures instead of drawing. At most 1024
//! characters are drawn.

use crate::window::Handle;
use core::sync::atomic::AtomicUsize;

const DT_CENTER: u32 = 0x1;
const DT_RIGHT: u32 = 0x2;
const DT_VCENTER: u32 = 0x4;
const DT_BOTTOM: u32 = 0x8;
const DT_WORDBREAK: u32 = 0x10;
const DT_SINGLELINE: u32 = 0x20;
const DT_NOCLIP: u32 = 0x100;
const DT_CALCRECT: u32 = 0x400;
const DT_NOPREFIX: u32 = 0x800;
const DT_END_ELLIPSIS: u32 = 0x8000;
const DT_PATH_ELLIPSIS: u32 = 0x4000;
const DT_WORD_ELLIPSIS: u32 = 0x40000;
const MAX: usize = 1024;

type Extent = extern "system" fn(Handle, *const u16, i32, *mut [i32; 2]) -> i32;
type Metrics = extern "system" fn(Handle, *mut [i32; 15]) -> i32;
type TextOut =
    extern "system" fn(Handle, i32, i32, u32, *const [i32; 4], *const u16, u32, *const i32) -> i32;
type Align = extern "system" fn(Handle, u32) -> u32;

struct Gdi {
    extent: Extent,
    metrics: Metrics,
    text_out: TextOut,
    align: Align,
}

fn gdi() -> Option<Gdi> {
    static E: AtomicUsize = AtomicUsize::new(0);
    static M: AtomicUsize = AtomicUsize::new(0);
    static T: AtomicUsize = AtomicUsize::new(0);
    static A: AtomicUsize = AtomicUsize::new(0);
    let f = |c: &AtomicUsize, n: &[u8]| crate::icon::gdi_proc(c, n);
    let (e, m, t, a) = (
        f(&E, b"GetTextExtentPoint32W\0"),
        f(&M, b"GetTextMetricsW\0"),
        f(&T, b"ExtTextOutW\0"),
        f(&A, b"SetTextAlign\0"),
    );
    if e == 0 || m == 0 || t == 0 || a == 0 {
        return None;
    }
    // SAFETY: gdi32's functions of those names have these signatures.
    unsafe {
        Some(Gdi {
            extent: core::mem::transmute::<usize, Extent>(e),
            metrics: core::mem::transmute::<usize, Metrics>(m),
            text_out: core::mem::transmute::<usize, TextOut>(t),
            align: core::mem::transmute::<usize, Align>(a),
        })
    }
}

fn width(g: &Gdi, hdc: Handle, s: &[u16]) -> i32 {
    let mut size = [0i32; 2];
    if s.is_empty() {
        return 0;
    }
    (g.extent)(hdc, s.as_ptr(), s.len() as i32, &mut size);
    size[0]
}

/// `s` cut so that it and "..." fit in `room` (the whole of `s` if it
/// fits): the characters kept and whether "..." follows.
fn ellipsis(g: &Gdi, hdc: Handle, s: &[u16], room: i32) -> (usize, bool) {
    if width(g, hdc, s) <= room {
        return (s.len(), false);
    }
    let dots = width(g, hdc, &[b'.' as u16; 3]);
    let mut n = s.len();
    while n > 0 && width(g, hdc, &s[..n]) + dots > room {
        n -= 1;
    }
    (n, true)
}

/// Splits `text` into lines (start, end) of at most `room` pixels when
/// `wrap`, else at line feeds only (or not at all for `single`).
fn lines(
    g: &Gdi,
    hdc: Handle,
    text: &[u16],
    single: bool,
    wrap: Option<i32>,
    out: &mut [(usize, usize); 128],
) -> usize {
    let mut n = 0;
    let mut push = |a: usize, b: usize, n: &mut usize| {
        if *n < out.len() {
            out[*n] = (a, b);
            *n += 1;
        }
    };
    if single {
        push(0, text.len(), &mut n);
        return n;
    }
    let mut start = 0;
    while start <= text.len() {
        let end = text[start..]
            .iter()
            .position(|&c| c == u16::from(b'\n'))
            .map_or(text.len(), |i| start + i);
        let mut line_end = end;
        if line_end > start && text[line_end - 1] == u16::from(b'\r') {
            line_end -= 1;
        }
        match wrap {
            Some(room) => {
                // Greedy: the longest run of words that fits; a word wider
                // than the room stands alone.
                let mut a = start;
                while a < line_end {
                    let mut b = a;
                    let mut last_fit = None;
                    while b < line_end {
                        let next = text[b..line_end]
                            .iter()
                            .position(|&c| c == u16::from(b' '))
                            .map_or(line_end, |i| b + i);
                        if width(g, hdc, &text[a..next]) > room && last_fit.is_some() {
                            break;
                        }
                        last_fit = Some(next);
                        b = next + 1;
                    }
                    let cut = last_fit.unwrap_or(line_end);
                    push(a, cut, &mut n);
                    a = cut + 1;
                    while a < line_end && text[a] == u16::from(b' ') {
                        a += 1;
                    }
                }
                if start == line_end {
                    push(start, start, &mut n);
                }
            }
            None => push(start, line_end, &mut n),
        }
        if end == text.len() {
            break;
        }
        start = end + 1;
    }
    n
}

/// DrawTextW(hdc, text, n, rect, format): the text's height; with
/// DT_VCENTER or DT_BOTTOM, the offset from the rectangle's top to the
/// text's bottom; 0 on failure.
#[unsafe(no_mangle)]
pub extern "system" fn DrawTextW(
    hdc: Handle,
    text: *const u16,
    n: i32,
    rect: *mut [i32; 4],
    format: u32,
) -> i32 {
    if text.is_null() || rect.is_null() {
        return 0;
    }
    let Some(g) = gdi() else { return 0 };
    // The characters, prefixes processed.
    let mut buf = [0u16; MAX];
    let mut len = 0;
    let mut i = 0usize;
    loop {
        if n >= 0 && i >= n as usize {
            break;
        }
        // SAFETY: the caller's `n` characters, or up to its NUL.
        let c = unsafe { *text.add(i) };
        if n < 0 && c == 0 {
            break;
        }
        i += 1;
        if format & DT_NOPREFIX == 0 && c == u16::from(b'&') {
            // "&&" is one '&'; a single '&' marks the next character.
            // SAFETY: as above.
            let next = if n >= 0 && i >= n as usize { 0 } else { unsafe { *text.add(i) } };
            if next == u16::from(b'&') {
                i += 1;
            } else {
                continue;
            }
        }
        if len < MAX {
            buf[len] = c;
            len += 1;
        }
    }
    let text = &buf[..len];
    // SAFETY: the caller's RECT.
    let r = unsafe { &mut *rect };
    let mut tm = [0i32; 15];
    (g.metrics)(hdc, &mut tm);
    let line_h = tm[0].max(1);
    let room = r[2] - r[0];
    let single = format & DT_SINGLELINE != 0;
    let wrap = (format & DT_WORDBREAK != 0 && !single).then_some(room);
    let mut spans = [(0usize, 0usize); 128];
    let count = lines(&g, hdc, text, single, wrap, &mut spans);
    let height = line_h * count as i32;
    if format & DT_CALCRECT != 0 {
        let widest =
            spans[..count].iter().map(|&(a, b)| width(&g, hdc, &text[a..b])).max().unwrap_or(0);
        r[2] = r[0] + widest;
        r[3] = r[1] + height;
        return height;
    }
    let top = if single && format & DT_VCENTER != 0 {
        r[1] + (r[3] - r[1] - height) / 2
    } else if single && format & DT_BOTTOM != 0 {
        r[3] - height
    } else {
        r[1]
    };
    let old_align = (g.align)(hdc, 0);
    let cut = format & (DT_END_ELLIPSIS | DT_PATH_ELLIPSIS | DT_WORD_ELLIPSIS) != 0;
    for (k, &(a, b)) in spans[..count].iter().enumerate() {
        let mut line = [0u16; MAX + 3];
        let (keep, dots) = if cut { ellipsis(&g, hdc, &text[a..b], room) } else { (b - a, false) };
        line[..keep].copy_from_slice(&text[a..a + keep]);
        let mut m = keep;
        if dots {
            line[m..m + 3].copy_from_slice(&[b'.' as u16; 3]);
            m += 3;
        }
        let w = width(&g, hdc, &line[..m]);
        let x = if format & DT_RIGHT != 0 {
            r[2] - w
        } else if format & DT_CENTER != 0 {
            r[0] + (room - w) / 2
        } else {
            r[0]
        };
        let options = if format & DT_NOCLIP != 0 { 0 } else { 4 }; // ETO_CLIPPED
        (g.text_out)(
            hdc,
            x,
            top + k as i32 * line_h,
            options,
            r,
            line.as_ptr(),
            m as u32,
            core::ptr::null(),
        );
    }
    (g.align)(hdc, old_align);
    if single && format & (DT_VCENTER | DT_BOTTOM) != 0 { top - r[1] + height } else { height }
}

/// DrawTextExW: DrawTextW (the DRAWTEXTPARAMS margins and tab stops are
/// not applied).
#[unsafe(no_mangle)]
pub extern "system" fn DrawTextExW(
    hdc: Handle,
    text: *mut u16,
    n: i32,
    rect: *mut [i32; 4],
    format: u32,
    _params: *const u8,
) -> i32 {
    DrawTextW(hdc, text, n, rect, format)
}

/// DrawTextA: the ANSI code page is UTF-8 here.
#[unsafe(no_mangle)]
pub extern "system" fn DrawTextA(
    hdc: Handle,
    text: *const u8,
    n: i32,
    rect: *mut [i32; 4],
    format: u32,
) -> i32 {
    if text.is_null() {
        return 0;
    }
    let len = if n >= 0 {
        n as usize
    } else {
        let mut k = 0;
        // SAFETY: the caller's NUL-terminated string.
        while unsafe { *text.add(k) } != 0 {
            k += 1;
        }
        k
    };
    // SAFETY: the caller's `len` bytes.
    let bytes = unsafe { core::slice::from_raw_parts(text, len) };
    let s = core::str::from_utf8(bytes).unwrap_or("?");
    let mut wide = [0u16; MAX];
    let mut m = 0;
    for c in s.encode_utf16().take(MAX) {
        wide[m] = c;
        m += 1;
    }
    DrawTextW(hdc, wide.as_ptr(), m as i32, rect, format)
}
