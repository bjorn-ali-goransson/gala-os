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
pub(crate) fn rop3(rop: u32, p: u32, s: u32, d: u32) -> u32 {
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
        // Both corners mapped (the extents may scale).
        let (x0, y0) = dc.device_point(x, y);
        let (x1, y1) = dc.device_point(x.saturating_add(w), y.saturating_add(h));
        let (dx, dy, w, h) = norm(x0, y0, x1 - x0, y1 - y0);
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
pub(crate) fn new_ddb(w: i32, h: i32, bpp: u16) -> Handle {
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
        palette: [0, 0x00ff_ffff],
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

/// CreateBitmapIndirect(BITMAP): CreateBitmap with its width, height,
/// planes, bits per pixel and bits.
#[unsafe(no_mangle)]
pub extern "system" fn CreateBitmapIndirect(bm: *const u8) -> Handle {
    if bm.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: the caller's BITMAP: bmType, bmWidth, bmHeight (i32 at 0,
    // 4, 8), bmWidthBytes (12), bmPlanes, bmBitsPixel (u16 at 16, 18),
    // bmBits (a pointer at 24).
    unsafe {
        let w = bm.add(4).cast::<i32>().read_unaligned();
        let h = bm.add(8).cast::<i32>().read_unaligned();
        let planes = bm.add(16).cast::<u16>().read_unaligned();
        let bpp = bm.add(18).cast::<u16>().read_unaligned();
        let bits = bm.add(24).cast::<*const u8>().read_unaligned();
        CreateBitmap(w, h, u32::from(planes), u32::from(bpp), bits)
    }
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

/// CreateDIBSection(hdc, info, usage, &bits, section, offset): a 1-, 24-
/// or 32-bit DIB (1-bit: with its two-colour palette) (BI_RGB, or BI_BITFIELDS with the usual masks) in memory of
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
    if ![1, 24, 32].contains(&h.bit_count) || (h.compression != 0 && h.compression != 3) {
        unsupported(b"gdi32.dll!CreateDIBSection(format)", u64::from(h.bit_count));
        return core::ptr::null_mut();
    }
    let (w, height) = (h.width, h.height.unsigned_abs() as i32);
    if w <= 0 || height == 0 || w > 16384 || height > 16384 {
        return core::ptr::null_mut();
    }
    let stride = ((w * i32::from(h.bit_count) + 31) / 32) * 4;
    // A 1-bit DIB's two colours (RGBQUADs after the header: blue, green,
    // red, reserved, which read as 0x00RRGGBB).
    let mut palette = [0, 0x00ff_ffff];
    if h.bit_count == 1 {
        let entries = if h.clr_used == 0 { 2 } else { (h.clr_used as usize).min(2) };
        for (i, e) in palette.iter_mut().enumerate().take(entries) {
            // SAFETY: the caller's colour table after the header.
            *e = unsafe {
                info.cast::<u8>().add(h.size as usize + 4 * i).cast::<u32>().read_unaligned()
            } & 0x00ff_ffff;
        }
    }
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
        palette,
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
/// text colour; underline and strike-out lines. `dx` gives each
/// character's advance instead of the font's (ExtTextOut's lpDx); `clip`
/// is a device rectangle the glyphs stay in (ETO_CLIPPED).
fn draw_text(
    dc: &Dc,
    f: &LogFont,
    x: i32,
    y: i32,
    text: &[char],
    dx: Option<&[i32]>,
    clip: Option<[i32; 4]>,
) {
    let m = font::metrics(f);
    let advance = |n: usize, c: char| match dx {
        Some(d) => d.get(n).copied().unwrap_or(0),
        None => m.advance(c),
    };
    let width: i32 = text.iter().enumerate().map(|(n, &c)| advance(n, c)).sum();
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
    let inside = |px: i32, py: i32| {
        clip.is_none_or(|[l, t, r, b]| px >= l && py >= t && px < r && py < b) && dc.visible(px, py)
    };
    if dc.mode == 2 {
        dc.fill(x, y, width, m.height, dc.background);
    }
    let mut gx = x;
    for (n, &c) in text.iter().enumerate() {
        m.glyph(c, |xx, yy, t| {
            let (px, py) = (gx + xx, y + yy);
            if inside(px, py) {
                let bg = dc.surface.get(px, py).unwrap_or(0);
                dc.surface.put(px, py, crate::surface::blend(bg, dc.text, t));
            }
        });
        gx += advance(n, c);
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
        draw_text(dc, &f, x, y, &buf[..count], None, None);
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
        draw_text(dc, &f, x, y, &buf[..count], None, None);
        1
    })
    .unwrap_or(0)
}

/// ExtTextOutW(hdc, x, y, options, rect, text, n, dx): ETO_OPAQUE (2)
/// fills `rect` with the background colour first, ETO_CLIPPED (4) keeps
/// the glyphs in it; `dx` gives each character's advance (ETO_PDY's
/// pairs are not supported). ETO_GLYPH_INDEX (0x10) text is drawn as
/// characters.
#[unsafe(no_mangle)]
pub extern "system" fn ExtTextOutW(
    hdc: Handle,
    x: i32,
    y: i32,
    options: u32,
    rect: *const [i32; 4],
    text: *const u16,
    n: u32,
    dx: *const i32,
) -> i32 {
    let Ok(n) = i32::try_from(n) else { return 0 };
    let Some(s) = wide(text, n) else { return 0 };
    let mut buf = ['\0'; 512];
    let count = chars(s, &mut buf);
    let dx = if dx.is_null() || options & 0x2000 != 0 {
        None
    } else {
        // SAFETY: the caller's array of `n` advances.
        Some(unsafe { core::slice::from_raw_parts(dx, n as usize) })
    };
    if options & 0x2000 != 0 && !dx.is_some() {
        crate::unsupported(b"gdi32.dll!ExtTextOutW ETO_PDY", u64::from(options));
    }
    // SAFETY: the caller's RECT when given.
    let rect = if rect.is_null() { None } else { Some(unsafe { *rect }) };
    with_dc(hdc, |dc, slots| {
        let device = rect.map(|[l, t, r, b]| {
            let (l, t) = dc.device_point(l, t);
            let (r, b) = dc.device_point(r, b);
            [l, t, r, b]
        });
        if let Some([l, t, r, b]) = device
            && options & 2 != 0
        {
            dc.fill(l, t, r - l, b - t, dc.background);
        }
        let f = font_of(dc, slots);
        let clip = if options & 4 != 0 { device } else { None };
        draw_text(dc, &f, x, y, &buf[..count], dx, clip);
        1
    })
    .unwrap_or(0)
}

/// GetTextExtentExPointW(hdc, text, n, max, fit, dx, size): the extent,
/// each prefix's width in `dx`, and in `fit` how many characters fit in
/// `max` (when `fit` is given).
#[unsafe(no_mangle)]
pub extern "system" fn GetTextExtentExPointW(
    hdc: Handle,
    text: *const u16,
    n: i32,
    max: i32,
    fit: *mut i32,
    dx: *mut i32,
    size: *mut [i32; 2],
) -> i32 {
    let Some(s) = wide(text, n) else { return 0 };
    if size.is_null() {
        return 0;
    }
    let mut buf = ['\0'; 512];
    let count = chars(s, &mut buf);
    with_dc(hdc, |dc, slots| {
        let m = font::metrics(&font_of(dc, slots));
        let (mut total, mut fits) = (0, 0);
        for (i, &c) in buf[..count].iter().enumerate() {
            total += m.advance(c);
            if total <= max {
                fits = i as i32 + 1;
            }
            if !dx.is_null() {
                // SAFETY: the caller's array of `n` widths.
                unsafe { dx.add(i).write(total) };
            }
        }
        // SAFETY: the caller's SIZE and count.
        unsafe {
            *size = [total, m.height];
            if !fit.is_null() {
                fit.write(fits);
            }
        }
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
    let mut buf = ['\0'; 512];
    let count = chars(s, &mut buf);
    with_dc(hdc, |dc, slots| {
        let m = font::metrics(&font_of(dc, slots));
        // SAFETY: the caller's SIZE.
        unsafe { *size = [m.width(&buf[..count]), m.height] };
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

// ---- For user32's icons ----

fn bitmap(h: Handle) -> Option<Bitmap> {
    let i = slot_of(h)?;
    with(|slots, _| match slots[i] {
        Object::Bitmap(b) => Some(b),
        _ => None,
    })
}

/// A copy of a bitmap (a device-dependent one of the same size and depth,
/// alpha bytes kept): what GetIconInfo hands out.
#[unsafe(no_mangle)]
pub extern "system" fn GalaCopyBitmap(h: Handle) -> Handle {
    let Some(b) = bitmap(h) else { return core::ptr::null_mut() };
    let s = b.surface;
    let copy = new_ddb(s.width, s.height, if b.bpp == 1 { 1 } else { 32 });
    let Some(c) = bitmap(copy) else { return copy };
    for y in 0..s.height {
        for x in 0..s.width {
            c.surface.put_raw(x, y, s.get_raw(x, y).unwrap_or(0));
        }
    }
    copy
}

/// GalaCutImage(src, sx, sy, sw, sh, mask, mx, my, key, w, h, out): a
/// w x h image (comctl32's image lists) from `src`'s rectangle (sx, sy,
/// sw, sh), scaled by the nearest pixel, as a 32-bit colour bitmap (alpha
/// bytes kept) and a monochrome mask (white: transparent). The mask comes
/// from `mask` at (mx, my) when given, else from the colour `key` (a
/// COLORREF; CLR_NONE: opaque); transparent pixels are black in the
/// colour bitmap. `out` receives [colour, mask].
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn GalaCutImage(
    src: Handle,
    sx: i32,
    sy: i32,
    sw: i32,
    sh: i32,
    mask: Handle,
    mx: i32,
    my: i32,
    key: u32,
    w: i32,
    h: i32,
    out: *mut [Handle; 2],
) -> i32 {
    let Some(s) = bitmap(src) else { return 0 };
    if out.is_null() || w <= 0 || h <= 0 || sw <= 0 || sh <= 0 {
        return 0;
    }
    let m = if mask.is_null() { None } else { bitmap(mask) };
    let key = (key != 0xffff_ffff).then(|| crate::pixel(key));
    let color = new_ddb(w, h, 32);
    let bits = new_ddb(w, h, 1);
    let (Some(c), Some(k)) = (bitmap(color), bitmap(bits)) else {
        crate::DeleteObject(color);
        crate::DeleteObject(bits);
        return 0;
    };
    for y in 0..h {
        for x in 0..w {
            let (ox, oy) = (x * sw / w, y * sh / h);
            let v = if s.bpp == 1 {
                s.surface.get(sx + ox, sy + oy).unwrap_or(0)
            } else {
                s.surface.get_raw(sx + ox, sy + oy).unwrap_or(0)
            };
            let clear = match (m, key) {
                (Some(m), _) => m.surface.get(mx + ox, my + oy).unwrap_or(0) & 0xff_ffff != 0,
                (None, Some(key)) => v & 0x00ff_ffff == key,
                (None, None) => false,
            };
            c.surface.put_raw(x, y, if clear { 0 } else { v });
            k.surface.put(x, y, if clear { 0x00ff_ffff } else { 0 });
        }
    }
    // SAFETY: the caller's two handles.
    unsafe { *out = [color, bits] };
    1
}

/// Draws an icon at logical (x, y), stretched to cx x cy (0: its size):
/// with a colour bitmap that has alpha, blended by it; otherwise the
/// destination ANDed with the mask, then XORed with the colour (a
/// monochrome icon: the mask's top half ANDs, its bottom half XORs).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn GalaDrawIcon(
    hdc: Handle,
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
    color: Handle,
    mask: Handle,
    _flags: u32,
) -> i32 {
    let Some(m) = bitmap(mask) else { return 0 };
    let c = bitmap(color);
    let (w, h) =
        (m.surface.width, if c.is_some() { m.surface.height } else { m.surface.height / 2 });
    if w <= 0 || h <= 0 {
        return 0;
    }
    let (cx, cy) = (if cx <= 0 { w } else { cx }, if cy <= 0 { h } else { cy });
    let alpha = c.is_some_and(|c| {
        c.surface.bpp == 32
            && (0..c.surface.height).any(|yy| {
                (0..c.surface.width).any(|xx| c.surface.get_raw(xx, yy).unwrap_or(0) >> 24 != 0)
            })
    });
    with_dc(hdc, |dc, _| {
        let (dx, dy) = dc.device_point(x, y);
        for j in 0..cy {
            for i in 0..cx {
                let (px, py) = (dx + i, dy + j);
                if !dc.visible(px, py) {
                    continue;
                }
                let (sx, sy) = (i * w / cx, j * h / cy);
                let d = dc.surface.get(px, py).unwrap_or(0);
                let out = match c {
                    Some(c) if alpha => {
                        let v = c.surface.get_raw(sx, sy).unwrap_or(0);
                        crate::surface::blend(d, v & 0x00ff_ffff, (v >> 24) as u8)
                    }
                    Some(c) => {
                        let and = m.surface.get(sx, sy).unwrap_or(0);
                        (d & and) ^ c.surface.get(sx, sy).unwrap_or(0)
                    }
                    None => {
                        let and = m.surface.get(sx, sy).unwrap_or(0);
                        (d & and) ^ m.surface.get(sx, sy + h).unwrap_or(0)
                    }
                };
                dc.surface.put(px, py, out);
            }
        }
        1
    })
    .unwrap_or(0)
}
