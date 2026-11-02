//! Device-independent bitmaps in the program's memory: reading them
//! (1/4/8 bits with a palette, 16 bits as 5-5-5, 24 and 32 bits; bottom-up
//! with a positive height, top-down with a negative one) for SetDIBits,
//! SetDIBitsToDevice and StretchDIBits, and writing them (1, 24 or 32
//! bits) for GetDIBits, which also fills in a header when asked with no
//! buffer and biBitCount 0.

use crate::draw::rop3;
use crate::{Handle, Object, slot_of, with, with_dc};

/// A DIB described by a BITMAPINFO.
struct Dib {
    bits: *mut u8,
    width: i32,
    height: i32,
    bottom_up: bool,
    bpp: u16,
    stride: usize,
    palette: *const u32,
    colors: usize,
}

impl Dib {
    /// Reads the BITMAPINFOHEADER (and finds the palette after it).
    fn new(info: *const u8, bits: *mut u8) -> Option<Dib> {
        if info.is_null() {
            return None;
        }
        // SAFETY: the caller's BITMAPINFO (a 40-byte header at least).
        unsafe {
            let size = info.cast::<u32>().read_unaligned() as usize;
            let width = info.add(4).cast::<i32>().read_unaligned();
            let height = info.add(8).cast::<i32>().read_unaligned();
            let bpp = info.add(14).cast::<u16>().read_unaligned();
            let compression = info.add(16).cast::<u32>().read_unaligned();
            let used = info.add(32).cast::<u32>().read_unaligned() as usize;
            if width <= 0
                || height == 0
                || ![1, 4, 8, 16, 24, 32].contains(&bpp)
                || (compression != 0 && compression != 3)
            {
                return None;
            }
            let colors =
                if bpp <= 8 { if used != 0 { used.min(256) } else { 1 << bpp } } else { 0 };
            // BI_BITFIELDS: three masks after the header (the usual ones).
            let palette = info.add(size + if compression == 3 { 12 } else { 0 }).cast::<u32>();
            Some(Dib {
                bits,
                width,
                height: height.abs(),
                bottom_up: height > 0,
                bpp,
                stride: (width as usize * bpp as usize).div_ceil(32) * 4,
                palette,
                colors,
            })
        }
    }

    fn row(&self, y: i32) -> *mut u8 {
        let r = if self.bottom_up { self.height - 1 - y } else { y };
        // SAFETY: callers keep y inside the DIB.
        unsafe { self.bits.add(r as usize * self.stride) }
    }

    fn get(&self, x: i32, y: i32) -> u32 {
        let p = self.row(y);
        // SAFETY: (x, y) inside the DIB; the palette has `colors` entries.
        unsafe {
            match self.bpp {
                32 => p.add(4 * x as usize).cast::<u32>().read_unaligned() & 0x00ff_ffff,
                24 => {
                    let q = p.add(3 * x as usize);
                    u32::from(*q) | u32::from(*q.add(1)) << 8 | u32::from(*q.add(2)) << 16
                }
                16 => {
                    let v = u32::from(p.add(2 * x as usize).cast::<u16>().read_unaligned());
                    let c = |shift: u32| ((v >> shift) & 0x1f) * 255 / 31;
                    c(10) << 16 | c(5) << 8 | c(0)
                }
                _ => {
                    let bit = x as usize * self.bpp as usize;
                    let byte = *p.add(bit / 8);
                    let index = (byte >> (8 - self.bpp as usize - bit % 8)) as usize
                        & ((1 << self.bpp) - 1);
                    if index < self.colors {
                        self.palette.add(index).read_unaligned() & 0x00ff_ffff
                    } else {
                        0
                    }
                }
            }
        }
    }

    fn put(&self, x: i32, y: i32, c: u32) {
        let p = self.row(y);
        // SAFETY: (x, y) inside the DIB.
        unsafe {
            match self.bpp {
                32 => p.add(4 * x as usize).cast::<u32>().write_unaligned(c & 0x00ff_ffff),
                24 => {
                    let q = p.add(3 * x as usize);
                    *q = c as u8;
                    *q.add(1) = (c >> 8) as u8;
                    *q.add(2) = (c >> 16) as u8;
                }
                1 => {
                    let q = p.add(x as usize / 8);
                    let bit = 0x80 >> (x % 8);
                    if crate::surface::mono(c) != 0 { *q |= bit } else { *q &= !bit }
                }
                _ => {}
            }
        }
    }
}

fn bitmap_surface(h: Handle) -> Option<crate::surface::Surface> {
    let i = slot_of(h)?;
    with(|slots, _| match slots[i] {
        Object::Bitmap(b) => Some((b.surface, b.bpp)),
        _ => None,
    })
    .map(|(s, _)| s)
}

fn bitmap_bpp(h: Handle) -> u16 {
    slot_of(h).map_or(32, |i| {
        with(|slots, _| match slots[i] {
            Object::Bitmap(b) => b.bpp,
            _ => 32,
        })
    })
}

/// GetDIBits(hdc, bitmap, start, lines, bits, info, usage): with no
/// buffer and biBitCount 0, the header (the bitmap's size and depth,
/// bottom-up); with a buffer, the scan lines [start, start + lines) in the
/// header's format (1, 24 or 32 bits); the lines copied.
#[unsafe(no_mangle)]
pub extern "system" fn GetDIBits(
    _hdc: Handle,
    bitmap: Handle,
    start: u32,
    lines: u32,
    bits: *mut u8,
    info: *mut u8,
    _usage: u32,
) -> i32 {
    let Some(s) = bitmap_surface(bitmap) else { return 0 };
    if info.is_null() {
        return 0;
    }
    // SAFETY: the caller's BITMAPINFO.
    let asked = unsafe { info.add(14).cast::<u16>().read_unaligned() };
    if bits.is_null() && asked == 0 {
        let bpp = bitmap_bpp(bitmap).max(1);
        let stride = (s.width as usize * bpp as usize).div_ceil(32) * 4;
        // SAFETY: as above (a 40-byte header).
        unsafe {
            info.add(4).cast::<i32>().write_unaligned(s.width);
            info.add(8).cast::<i32>().write_unaligned(s.height);
            info.add(12).cast::<u16>().write_unaligned(1);
            info.add(14).cast::<u16>().write_unaligned(bpp);
            info.add(16).cast::<u32>().write_unaligned(0);
            info.add(20).cast::<u32>().write_unaligned((stride * s.height as usize) as u32);
        }
        return s.height;
    }
    if bits.is_null() {
        return s.height;
    }
    let Some(d) = Dib::new(info, bits) else { return 0 };
    if ![1, 24, 32].contains(&d.bpp) {
        crate::unsupported(b"gdi32.dll!GetDIBits(format)", u64::from(d.bpp));
        return 0;
    }
    if d.bpp == 1 {
        // The two colours: black and white.
        // SAFETY: room for a 2-entry palette after the header.
        unsafe {
            let pal = info.add(40).cast::<u32>();
            pal.write_unaligned(0);
            pal.add(1).write_unaligned(0x00ff_ffff);
        }
    }
    // Scan lines count from the bottom.
    let end = (start + lines).min(d.height as u32).min(s.height as u32);
    for line in start..end {
        let sy = s.height - 1 - line as i32;
        let dy = d.height - 1 - line as i32;
        for x in 0..d.width.min(s.width) {
            d.put(x, dy, s.get(x, sy).unwrap_or(0));
        }
    }
    end.saturating_sub(start) as i32
}

/// SetDIBits(hdc, bitmap, start, lines, bits, info, usage): writes scan
/// lines (from the bottom) of the DIB into the bitmap; the lines written.
#[unsafe(no_mangle)]
pub extern "system" fn SetDIBits(
    _hdc: Handle,
    bitmap: Handle,
    start: u32,
    lines: u32,
    bits: *const u8,
    info: *const u8,
    _usage: u32,
) -> i32 {
    let Some(s) = bitmap_surface(bitmap) else { return 0 };
    let Some(d) = Dib::new(info, bits.cast_mut()) else { return 0 };
    let end = (start + lines).min(d.height as u32).min(s.height as u32);
    for line in start..end {
        let sy = s.height - 1 - line as i32;
        let dy = d.height - 1 - line as i32;
        for x in 0..d.width.min(s.width) {
            s.put(x, sy, d.get(x, dy));
        }
    }
    end.saturating_sub(start) as i32
}

/// StretchDIBits: the DIB's source rectangle scaled onto the DC's
/// destination rectangle with a raster operation; the lines drawn.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn StretchDIBits(
    hdc: Handle,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    sx: i32,
    sy: i32,
    sw: i32,
    sh: i32,
    bits: *const u8,
    info: *const u8,
    _usage: u32,
    rop: u32,
) -> i32 {
    let Some(d) = Dib::new(info, bits.cast_mut()) else { return 0 };
    if w == 0 || h == 0 || sw == 0 || sh == 0 {
        return 0;
    }
    // Source y counts from the bottom in a bottom-up DIB's terms: the
    // documented origin is the DIB's lower-left corner when bottom-up.
    with_dc(hdc, |dc, slots| {
        let p = match slots[dc.brush] {
            Object::Brush { color, null: false } => color,
            _ => 0,
        };
        let (dx0, dy0) = dc.device_point(x, y);
        let (aw, ah) = (w.abs(), h.abs());
        for j in 0..ah {
            let ty = if h < 0 { dy0 - 1 - j } else { dy0 + j };
            let fy = sy + j * sh / ah;
            let src_y = if d.bottom_up { d.height - sh - sy + (fy - sy) } else { fy };
            if src_y < 0 || src_y >= d.height {
                continue;
            }
            for i in 0..aw {
                let tx = if w < 0 { dx0 - 1 - i } else { dx0 + i };
                let fx = sx + i * sw / aw;
                if fx < 0 || fx >= d.width || !dc.visible(tx, ty) {
                    continue;
                }
                let s = d.get(fx, src_y);
                let dst = dc.surface.get(tx, ty).unwrap_or(0);
                dc.surface.put(tx, ty, rop3(rop, p, s, dst));
            }
        }
        ah
    })
    .unwrap_or(0)
}

/// SetDIBitsToDevice: the DIB's rectangle at (x, y), unscaled (SRCCOPY);
/// the lines drawn.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn SetDIBitsToDevice(
    hdc: Handle,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    sx: i32,
    sy: i32,
    _start: u32,
    lines: u32,
    bits: *const u8,
    info: *const u8,
    usage: u32,
) -> i32 {
    let drawn = StretchDIBits(
        hdc,
        x,
        y,
        w as i32,
        h as i32,
        sx,
        sy,
        w as i32,
        h as i32,
        bits,
        info,
        usage,
        0x00cc_0020,
    );
    drawn.min(lines as i32)
}

/// GdiFlush: gala does not batch GDI calls.
#[unsafe(no_mangle)]
pub extern "system" fn GdiFlush() -> i32 {
    1
}

/// CreateDIBitmap(hdc, header, init, bits, info, usage): a DDB of the
/// header's size in `hdc`'s format (the screen's without a DC); with
/// CBM_INIT (4), the DIB's bits set as SetDIBits does.
#[unsafe(no_mangle)]
pub extern "system" fn CreateDIBitmap(
    hdc: Handle,
    header: *const u8,
    init: u32,
    bits: *const u8,
    info: *const u8,
    usage: u32,
) -> Handle {
    if header.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: the caller's BITMAPINFOHEADER: biWidth at 4, biHeight at 8.
    let (w, h) = unsafe {
        (header.add(4).cast::<i32>().read_unaligned(), header.add(8).cast::<i32>().read_unaligned())
    };
    let h = h.saturating_abs();
    if w <= 0 || h == 0 {
        return core::ptr::null_mut();
    }
    let b = if hdc.is_null() {
        crate::draw::new_ddb(w, h, 32)
    } else {
        crate::draw::CreateCompatibleBitmap(hdc, w, h)
    };
    if !b.is_null() && init & 4 != 0 && !bits.is_null() && !info.is_null() {
        SetDIBits(hdc, b, 0, h as u32, bits, info, usage);
    }
    b
}

/// Get/SetDIBColorTable on the DIB section selected into `hdc`: entries
/// `start..start + n` of its colour table (RGBQUADs, which read as
/// 0x00RRGGBB) copied out or in; the number done (0 for a bitmap
/// without a table: only 1-bit DIB sections have one here).
fn color_table(hdc: Handle, start: u32, n: u32, table: *mut u32, set: bool) -> u32 {
    if table.is_null() {
        return 0;
    }
    with_dc(hdc, |dc, slots| {
        let Object::Bitmap(mut b) = slots[dc.bitmap] else { return 0 };
        if !b.dib || b.surface.bpp != 1 {
            return 0;
        }
        let first = start as usize;
        let count = (n as usize).min(2usize.saturating_sub(first));
        for i in 0..count {
            // SAFETY: the caller's array of `n` RGBQUADs.
            unsafe {
                let e = table.add(i);
                if set {
                    b.surface.palette[first + i] = e.read_unaligned() & 0x00ff_ffff;
                } else {
                    e.write_unaligned(b.surface.palette[first + i]);
                }
            }
        }
        if set {
            // The DC draws through its own copy of the surface.
            dc.surface.palette = b.surface.palette;
            slots[dc.bitmap] = Object::Bitmap(b);
        }
        count as u32
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetDIBColorTable(hdc: Handle, start: u32, n: u32, table: *mut u32) -> u32 {
    color_table(hdc, start, n, table, false)
}

#[unsafe(no_mangle)]
pub extern "system" fn SetDIBColorTable(hdc: Handle, start: u32, n: u32, table: *const u32) -> u32 {
    color_table(hdc, start, n, table.cast_mut(), true)
}
