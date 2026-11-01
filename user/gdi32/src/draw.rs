//! Drawing: PatBlt, Rectangle, pixels, lines, bitmaps (device-dependent
//! ones and DIB sections), BitBlt/StretchBlt with any ternary raster
//! operation, and text with the DC's font.

use crate::font::{self, LogFont};
use crate::surface::{Surface, mono};
use crate::{
    Bitmap, Dc, DcKind, Handle, Object, alloc, new_object, pixel, slot_of, unsupported, with,
    with_dc,
};

/// The DC's brush colour (None: the null brush).
fn brush(dc: &Dc, slots: &[Object]) -> Option<u32> {
    match slots[dc.brush] {
        Object::Brush { color, null: false } => Some(color),
        _ => None,
    }
}

fn pen(dc: &Dc, slots: &[Object]) -> Option<(u32, i32)> {
    match slots[dc.pen] {
        Object::Pen { color, null: false, width, .. } => Some((color, width)),
        _ => None,
    }
}

/// Normalizes a rectangle given by a corner and a (possibly negative) size.
fn norm(x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
    let (x, w) = if w < 0 { (x + w, -w) } else { (x, w) };
    let (y, h) = if h < 0 { (y + h, -h) } else { (y, h) };
    (x, y, w, h)
}

/// A ternary raster operation's result for pattern `p`, source `s` and
/// destination `d` (bitwise): the operation's index (bits 16-23) is the
/// truth table over (P, S, D).
fn rop3(rop: u32, p: u32, s: u32, d: u32) -> u32 {
    let table = (rop >> 16) & 0xff;
    let mut out = 0;
    for i in 0..8 {
        if table >> i & 1 != 0 {
            let pp = if i & 4 != 0 { p } else { !p };
            let ss = if i & 2 != 0 { s } else { !s };
            let dd = if i & 1 != 0 { d } else { !d };
            out |= pp & ss & dd;
        }
    }
    out & 0x00ff_ffff
}

/// Whether the operation reads the source.
fn uses_source(rop: u32) -> bool {
    let t = (rop >> 16) & 0xff;
    ((t >> 2) & 0x33) != (t & 0x33)
}

#[unsafe(no_mangle)]
pub extern "system" fn PatBlt(hdc: Handle, x: i32, y: i32, w: i32, h: i32, rop: u32) -> i32 {
    if uses_source(rop) {
        return 0;
    }
    with_dc(hdc, |dc, slots| {
        let p = brush(dc, slots).unwrap_or(0);
        let (x, y, w, h) = norm(x, y, w, h);
        let (dx, dy) = dc.device_point(x, y);
        for yy in dy..dy + h {
            for xx in dx..dx + w {
                if dc.visible(xx, yy) {
                    let d = dc.surface.get(xx, yy).unwrap_or(0);
                    dc.surface.put(xx, yy, rop3(rop, p, 0, d));
                }
            }
        }
        1
    })
    .unwrap_or(0)
}

/// Rectangle(hdc, left, top, right, bottom): the brush inside, the pen's
/// outline (its width inward).
#[unsafe(no_mangle)]
pub extern "system" fn Rectangle(hdc: Handle, l: i32, t: i32, r: i32, b: i32) -> i32 {
    with_dc(hdc, |dc, slots| {
        let (x0, y0) = dc.device_point(l, t);
        let (x1, y1) = dc.device_point(r, b);
        if let Some(c) = brush(dc, slots) {
            dc.fill(x0, y0, x1 - x0, y1 - y0, c);
        }
        if let Some((c, w)) = pen(dc, slots) {
            dc.fill(x0, y0, x1 - x0, w, c);
            dc.fill(x0, y1 - w, x1 - x0, w, c);
            dc.fill(x0, y0, w, y1 - y0, c);
            dc.fill(x1 - w, y0, w, y1 - y0, c);
        }
        1
    })
    .unwrap_or(0)
}

const CLR_INVALID: u32 = 0xffff_ffff;

/// SetPixel: the colour set (a monochrome bitmap's black or white), or
/// CLR_INVALID outside the surface or clip.
#[unsafe(no_mangle)]
pub extern "system" fn SetPixel(hdc: Handle, x: i32, y: i32, color: u32) -> u32 {
    with_dc(hdc, |dc, _| {
        let (x, y) = dc.device_point(x, y);
        if !dc.visible(x, y) {
            return CLR_INVALID;
        }
        let c = pixel(color);
        dc.surface.put(x, y, c);
        pixel(if dc.surface.mono { mono(c) } else { c })
    })
    .unwrap_or(CLR_INVALID)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetPixel(hdc: Handle, x: i32, y: i32) -> u32 {
    with_dc(hdc, |dc, _| {
        let (x, y) = dc.device_point(x, y);
        if !dc.visible(x, y) {
            return CLR_INVALID;
        }
        dc.surface.get(x, y).map_or(CLR_INVALID, pixel)
    })
    .unwrap_or(CLR_INVALID)
}

#[unsafe(no_mangle)]
pub extern "system" fn MoveToEx(hdc: Handle, x: i32, y: i32, prev: *mut [i32; 2]) -> i32 {
    with_dc(hdc, |dc, _| {
        if !prev.is_null() {
            // SAFETY: the caller's POINT.
            unsafe { *prev = [dc.pos.0, dc.pos.1] };
        }
        dc.pos = (x, y);
        1
    })
    .unwrap_or(0)
}

/// LineTo: a line from the current position to (x, y), the end point
/// excluded (Bresenham), in the pen's colour and width.
#[unsafe(no_mangle)]
pub extern "system" fn LineTo(hdc: Handle, x: i32, y: i32) -> i32 {
    with_dc(hdc, |dc, slots| {
        let (x0, y0) = dc.device_point(dc.pos.0, dc.pos.1);
        let (x1, y1) = dc.device_point(x, y);
        dc.pos = (x, y);
        let Some((c, w)) = pen(dc, slots) else { return 1 };
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let (mut px, mut py, mut err) = (x0, y0, dx + dy);
        while (px, py) != (x1, y1) {
            dc.fill(px - (w - 1) / 2, py - (w - 1) / 2, w, w, c);
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                px += sx;
            }
            if e2 <= dx {
                err += dx;
                py += sy;
            }
        }
        1
    })
    .unwrap_or(0)
}

// ---- Bitmaps ----

/// A 32-bit device-dependent bitmap (shown as `bpp` deep), black.
fn new_ddb(w: i32, h: i32, bpp: u16) -> Handle {
    if w <= 0 || h <= 0 || w > 16384 || h > 16384 {
        return core::ptr::null_mut();
    }
    let bits = alloc(w as usize * h as usize * 4);
    if bits.is_null() {
        return core::ptr::null_mut();
    }
    let surface = Surface {
        bits,
        width: w,
        height: h,
        stride: w * 4,
        bpp: 32,
        bottom_up: false,
        mono: bpp == 1,
    };
    new_object(Object::Bitmap(Bitmap { surface, bpp, dib: false, owned: true }))
}

/// CreateCompatibleBitmap: like the DC's surface (a memory DC with the
/// default 1 x 1 bitmap: monochrome, as documented); 0 x 0: a 1 x 1
/// monochrome bitmap.
#[unsafe(no_mangle)]
pub extern "system" fn CreateCompatibleBitmap(hdc: Handle, w: i32, h: i32) -> Handle {
    let Some(is_mono) = with_dc(hdc, |dc, _| dc.kind == DcKind::Memory && dc.surface.mono) else {
        return core::ptr::null_mut();
    };
    if w == 0 || h == 0 {
        return new_ddb(1, 1, 1);
    }
    new_ddb(w, h, if is_mono { 1 } else { 32 })
}

/// CreateBitmap(w, h, planes, bpp, bits): 1-bit (rows padded to 16 bits,
/// set bits white) or 32-bit bitmaps; others are kept as 32-bit.
#[unsafe(no_mangle)]
pub extern "system" fn CreateBitmap(
    w: i32,
    h: i32,
    _planes: u32,
    bpp: u32,
    bits: *const u8,
) -> Handle {
    let shown = if bpp == 1 { 1 } else { 32 };
    let hb = new_ddb(w, h, shown);
    if hb.is_null() || bits.is_null() {
        return hb;
    }
    let Some(i) = slot_of(hb) else { return hb };
    let surface = with(|slots, _| match slots[i] {
        Object::Bitmap(b) => b.surface,
        _ => Surface::NONE,
    });
    for y in 0..h {
        for x in 0..w {
            // SAFETY: the caller's bits for w x h at this depth.
            let c = unsafe {
                match bpp {
                    1 => {
                        let row = ((w + 15) / 16) * 2;
                        let byte = *bits.add((y * row + x / 8) as usize);
                        if byte & (0x80 >> (x % 8)) != 0 { 0x00ff_ffff } else { 0 }
                    }
                    32 => {
                        bits.add(((y * w + x) * 4) as usize).cast::<u32>().read_unaligned()
                            & 0x00ff_ffff
                    }
                    _ => 0,
                }
            };
            surface.put(x, y, c);
        }
    }
    hb
}

/// BITMAPINFOHEADER's fields that matter here.
#[repr(C)]
pub struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_ppm: i32,
    y_ppm: i32,
    clr_used: u32,
    clr_important: u32,
}

/// CreateDIBSection(hdc, info, usage, &bits, section, offset): a 24- or
/// 32-bit DIB (BI_RGB, or BI_BITFIELDS with the usual masks) in memory of
/// its own (file mappings are not supported); `bits` receives it.
#[unsafe(no_mangle)]
pub extern "system" fn CreateDIBSection(
    _hdc: Handle,
    info: *const BitmapInfoHeader,
    _usage: u32,
    bits: *mut *mut u8,
    section: Handle,
    _offset: u32,
) -> Handle {
    if info.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: the caller's BITMAPINFO.
    let h = unsafe { &*info };
    if !section.is_null() {
        unsupported(b"gdi32.dll!CreateDIBSection(section)", 0);
        return core::ptr::null_mut();
    }
    if (h.bit_count != 24 && h.bit_count != 32) || (h.compression != 0 && h.compression != 3) {
        unsupported(b"gdi32.dll!CreateDIBSection(format)", u64::from(h.bit_count));
        return core::ptr::null_mut();
    }
    let (w, height) = (h.width, h.height.unsigned_abs() as i32);
    if w <= 0 || height == 0 || w > 16384 || height > 16384 {
        return core::ptr::null_mut();
    }
    let stride = ((w * i32::from(h.bit_count) + 31) / 32) * 4;
    let memory = alloc(stride as usize * height as usize);
    if memory.is_null() {
        return core::ptr::null_mut();
    }
    let surface = Surface {
        bits: memory,
        width: w,
        height,
        stride,
        bpp: h.bit_count,
        bottom_up: h.height > 0,
        mono: false,
    };
    let hb =
        new_object(Object::Bitmap(Bitmap { surface, bpp: h.bit_count, dib: true, owned: true }));
    if !bits.is_null() {
        // SAFETY: the caller's pointer.
        unsafe { *bits = if hb.is_null() { core::ptr::null_mut() } else { memory } };
    }
    hb
}

/// The source DC's state for a blit (None: not a DC).
fn source(hdc: Handle) -> Option<Dc> {
    let i = slot_of(hdc)?;
    with(|slots, _| match slots[i] {
        Object::Dc(d) => Some(d),
        _ => None,
    })
}

/// BitBlt: StretchBlt without stretching.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn BitBlt(
    dst: Handle,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    src: Handle,
    sx: i32,
    sy: i32,
    rop: u32,
) -> i32 {
    StretchBlt(dst, x, y, w, h, src, sx, sy, w, h, rop)
}

/// StretchBlt: each destination pixel from the nearest source pixel
/// (negative sizes mirror), combined by the raster operation with the
/// brush and the destination. Overlapping copies within one surface read
/// a snapshot of the source rows first when they overlap.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn StretchBlt(
    dst: Handle,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    src: Handle,
    sx: i32,
    sy: i32,
    sw: i32,
    sh: i32,
    rop: u32,
) -> i32 {
    let needs_source = uses_source(rop);
    let s = if needs_source {
        match source(src) {
            Some(s) => Some(s),
            None => return 0,
        }
    } else {
        None
    };
    if w == 0 || h == 0 || (needs_source && (sw == 0 || sh == 0)) {
        return 1;
    }
    with_dc(dst, |dc, slots| {
        let p = brush(dc, slots).unwrap_or(0);
        let (dx0, dy0) = dc.device_point(x, y);
        let (ssx, ssy) = s.map_or((0, 0), |s| s.device_point(sx, sy));
        // Same surface, overlapping: go in the direction that reads source
        // pixels before they are written (plain copies only, no stretch).
        let same = s.is_some_and(|s| s.surface.bits == dc.surface.bits);
        let backwards_y = same && dy0 > ssy;
        let backwards_x = same && dx0 > ssx;
        let (aw, ah) = (w.abs(), h.abs());
        for j in 0..ah {
            let j = if backwards_y { ah - 1 - j } else { j };
            let ty = if h < 0 { dy0 - 1 - j } else { dy0 + j };
            for i in 0..aw {
                let i = if backwards_x { aw - 1 - i } else { i };
                let tx = if w < 0 { dx0 - 1 - i } else { dx0 + i };
                if !dc.visible(tx, ty) {
                    continue;
                }
                let sv = match s {
                    Some(s) => {
                        let fx = ssx + (i * sw.abs() / aw) * sw.signum();
                        let fy = ssy + (j * sh.abs() / ah) * sh.signum();
                        s.surface.get(fx, fy).unwrap_or(0)
                    }
                    None => 0,
                };
                let d = dc.surface.get(tx, ty).unwrap_or(0);
                dc.surface.put(tx, ty, rop3(rop, p, sv, d));
            }
        }
        1
    })
    .unwrap_or(0)
}

// ---- Text ----

fn font_of(dc: &Dc, slots: &[Object]) -> LogFont {
    match slots[dc.font] {
        Object::Font(f) => f,
        _ => LogFont::default_gui(),
    }
}

/// SetTextAlign: TA_LEFT/RIGHT/CENTER (0/2/6) and TA_TOP/BOTTOM/BASELINE
/// (0/8/24); the previous value.
#[unsafe(no_mangle)]
pub extern "system" fn SetTextAlign(hdc: Handle, align: u32) -> u32 {
    with_dc(hdc, |dc, _| core::mem::replace(&mut dc.align, align)).unwrap_or(0xffff_ffff)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetTextAlign(hdc: Handle) -> u32 {
    with_dc(hdc, |dc, _| dc.align).unwrap_or(0xffff_ffff)
}

/// Draws `text` at logical (x, y) by the alignment: OPAQUE fills the
/// cells with the background first; each glyph's coverage blends the
/// text colour; underline and strike-out lines.
fn draw_text(dc: &Dc, f: &LogFont, x: i32, y: i32, text: &[char]) {
    let m = font::metrics(f);
    let width = m.advance * text.len() as i32;
    let x = match dc.align & 6 {
        2 => x - width,
        6 => x - width / 2,
        _ => x,
    };
    let y = match dc.align & 24 {
        8 => y - m.height,
        24 => y - m.ascent,
        _ => y,
    };
    let (x, y) = dc.device_point(x, y);
    if dc.mode == 2 {
        dc.fill(x, y, width, m.height, dc.background);
    }
    for (n, &c) in text.iter().enumerate() {
        let gx = x + n as i32 * m.advance;
        for yy in 0..m.height {
            for xx in 0..m.advance {
                let t = m.coverage(c, xx, yy);
                let (px, py) = (gx + xx, y + yy);
                if t != 0 && dc.visible(px, py) {
                    let bg = dc.surface.get(px, py).unwrap_or(0);
                    dc.surface.put(px, py, crate::surface::blend(bg, dc.text, t));
                }
            }
        }
    }
    let thickness = (m.height / 16).max(1);
    if m.underline {
        dc.fill(x, y + m.ascent + 1, width, thickness, dc.text);
    }
    if m.strike_out {
        dc.fill(x, y + m.ascent * 2 / 3, width, thickness, dc.text);
    }
}

/// UTF-16 to characters (at most `out.len()`).
fn chars(text: &[u16], out: &mut [char]) -> usize {
    let mut n = 0;
    for c in char::decode_utf16(text.iter().copied()) {
        if n == out.len() {
            break;
        }
        out[n] = c.unwrap_or('?');
        n += 1;
    }
    n
}

fn wide<'a>(text: *const u16, n: i32) -> Option<&'a [u16]> {
    if n < 0 || (text.is_null() && n > 0) {
        return None;
    }
    // SAFETY: the caller's `n` characters.
    Some(if n == 0 { &[] } else { unsafe { core::slice::from_raw_parts(text, n as usize) } })
}

#[unsafe(no_mangle)]
pub extern "system" fn TextOutW(hdc: Handle, x: i32, y: i32, text: *const u16, n: i32) -> i32 {
    let Some(s) = wide(text, n) else { return 0 };
    let mut buf = ['\0'; 512];
    let count = chars(s, &mut buf);
    with_dc(hdc, |dc, slots| {
        let f = font_of(dc, slots);
        draw_text(dc, &f, x, y, &buf[..count]);
        1
    })
    .unwrap_or(0)
}

/// TextOutA: the ANSI code page is UTF-8 here.
#[unsafe(no_mangle)]
pub extern "system" fn TextOutA(hdc: Handle, x: i32, y: i32, text: *const u8, n: i32) -> i32 {
    if n < 0 || (text.is_null() && n > 0) {
        return 0;
    }
    // SAFETY: the caller's `n` bytes.
    let s = if n == 0 { &[][..] } else { unsafe { core::slice::from_raw_parts(text, n as usize) } };
    let text = core::str::from_utf8(s).unwrap_or("?");
    let mut buf = ['\0'; 512];
    let mut count = 0;
    for c in text.chars().take(buf.len()) {
        buf[count] = c;
        count += 1;
    }
    with_dc(hdc, |dc, slots| {
        let f = font_of(dc, slots);
        draw_text(dc, &f, x, y, &buf[..count]);
        1
    })
    .unwrap_or(0)
}

/// GetTextExtentPoint32W: the characters' advances by the font's height.
#[unsafe(no_mangle)]
pub extern "system" fn GetTextExtentPoint32W(
    hdc: Handle,
    text: *const u16,
    n: i32,
    size: *mut [i32; 2],
) -> i32 {
    let Some(s) = wide(text, n) else { return 0 };
    if size.is_null() {
        return 0;
    }
    let count = char::decode_utf16(s.iter().copied()).count() as i32;
    with_dc(hdc, |dc, slots| {
        let m = font::metrics(&font_of(dc, slots));
        // SAFETY: the caller's SIZE.
        unsafe { *size = [count * m.advance, m.height] };
        1
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetTextMetricsW(hdc: Handle, out: *mut font::TextMetricW) -> i32 {
    if out.is_null() {
        return 0;
    }
    with_dc(hdc, |dc, slots| {
        // SAFETY: the caller's TEXTMETRICW.
        unsafe { out.write(font::text_metrics(&font_of(dc, slots))) };
        1
    })
    .unwrap_or(0)
}

/// GetTextFaceW(hdc, n, buf): the selected font's face name; the
/// characters copied with the NUL (the length needed with the NUL when
/// `buf` is NULL).
#[unsafe(no_mangle)]
pub extern "system" fn GetTextFaceW(hdc: Handle, n: i32, buf: *mut u16) -> i32 {
    with_dc(hdc, |dc, slots| {
        let f = font_of(dc, slots);
        let len = f.face.iter().position(|&c| c == 0).unwrap_or(f.face.len());
        if buf.is_null() {
            return len as i32 + 1;
        }
        let k = len.min((n.max(1) - 1) as usize);
        // SAFETY: the caller's buffer of `n` characters.
        unsafe {
            core::ptr::copy_nonoverlapping(f.face.as_ptr(), buf, k);
            *buf.add(k) = 0;
        }
        k as i32 + 1
    })
    .unwrap_or(0)
}
