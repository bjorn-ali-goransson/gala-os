//! The BMP decoder's reading of a file (Microsoft's BITMAPFILEHEADER,
//! BITMAPCOREHEADER, BITMAPINFOHEADER, BITMAPV4HEADER and BITMAPV5HEADER
//! documentation; native formats and resolutions from Wine's bmpformat
//! test, run on Windows):
//! - 1, 4 and 8 bpp: 1/4/8bppIndexed, the colour table (RGBTRIPLEs after
//!   a core header, RGBQUADs otherwise; biClrUsed entries, or all) as
//!   opaque colours; 16 bpp: 16bppBGR555, or BGR565 when its bit fields
//!   say so; 24 bpp: 24bppBGR; 32 bpp: 32bppBGR, the bytes as they are
//!   (BI_RGB) or each pixel's fields moved to B, G, R (BI_BITFIELDS, the
//!   fourth byte 0).
//! - Rows bottom-up unless the height is negative; each row padded to
//!   four bytes in the file.
//! - The resolution is the header's pixels per metre in dpi (x 0.0254)
//!   when both are set, else 96 x 96 (a core header has none).
//!
//! Not decoded (recorded, WINCODEC_ERR_UNSUPPORTEDOPERATION): RLE4 and
//! RLE8, JPEG and PNG inside a BMP, 16 bpp bit fields other than 555 and
//! 565, 32 bpp bit fields of other widths than 8.

use alloc::vec::Vec;

use crate::decoder::Image;
use crate::format;
use crate::{
    E_OUTOFMEMORY, HResult, WINCODEC_ERR_BADIMAGE, WINCODEC_ERR_UNKNOWNIMAGEFORMAT,
    WINCODEC_ERR_UNSUPPORTEDOPERATION, gap, row_bytes, zeroed,
};

const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;

fn le16(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_le_bytes(d.get(at..at + 2)?.try_into().ok()?)))
}

fn le32(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

/// A field of `v` under `mask`, as 8 bits.
fn field(v: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let width = (mask >> shift).trailing_ones();
    let x = (v & mask) >> shift;
    if width >= 8 { (x >> (width - 8)) as u8 } else { (x * 255 / ((1 << width) - 1)) as u8 }
}

/// Whether `data` starts like a BMP file.
pub fn sniff(data: &[u8]) -> bool {
    data.starts_with(b"BM")
}

pub fn decode(data: &[u8]) -> Result<Image, HResult> {
    let bad = WINCODEC_ERR_BADIMAGE;
    if !sniff(data) {
        return Err(WINCODEC_ERR_UNKNOWNIMAGEFORMAT);
    }
    let bits_at = le32(data, 10).ok_or(bad)? as usize;
    let hsize = le32(data, 14).ok_or(bad)? as usize;
    let core = hsize == 12;
    let (width, height, bpp, compression, xppm, yppm, used) = if core {
        let w = le16(data, 18).ok_or(bad)? as i32;
        let h = le16(data, 20).ok_or(bad)? as i32;
        (w, h, le16(data, 24).ok_or(bad)?, BI_RGB, 0, 0, 0)
    } else if hsize >= 40 {
        (
            le32(data, 18).ok_or(bad)? as i32,
            le32(data, 22).ok_or(bad)? as i32,
            le16(data, 28).ok_or(bad)?,
            le32(data, 30).ok_or(bad)?,
            le32(data, 38).ok_or(bad)?,
            le32(data, 42).ok_or(bad)?,
            le32(data, 46).ok_or(bad)?,
        )
    } else {
        return Err(bad);
    };
    if width <= 0 || height == 0 {
        return Err(bad);
    }
    let (w, h) = (width as u32, height.unsigned_abs());
    let top_down = height < 0;
    // The bit fields: in the header (V2 and later) or right after a
    // BITMAPINFOHEADER.
    let masks = if compression == BI_BITFIELDS {
        let at = 14 + 40;
        Some((
            le32(data, at).ok_or(bad)?,
            le32(data, at + 4).ok_or(bad)?,
            le32(data, at + 8).ok_or(bad)?,
        ))
    } else {
        None
    };
    if compression != BI_RGB && compression != BI_BITFIELDS {
        gap(b"BMP decoder(compression)", compression.into());
        return Err(WINCODEC_ERR_UNSUPPORTEDOPERATION);
    }
    let fmt = match (bpp, masks) {
        (1, None) => &format::FORMATS[0],
        (4, None) => &format::FORMATS[2],
        (8, None) => &format::FORMATS[3],
        (16, None) | (16, Some((0x7c00, 0x03e0, 0x001f))) => {
            format::find(&format::BGR555).ok_or(bad)?
        }
        (16, Some((0xf800, 0x07e0, 0x001f))) => format::find(&format::BGR565).ok_or(bad)?,
        (24, None) => format::find(&format::BGR24).ok_or(bad)?,
        (32, None) => format::find(&format::BGR32).ok_or(bad)?,
        (32, Some((r, g, b)))
            if [r, g, b]
                .iter()
                .all(|m| *m != 0 && (m >> m.trailing_zeros()).trailing_ones() == 8) =>
        {
            format::find(&format::BGR32).ok_or(bad)?
        }
        _ => {
            gap(b"BMP decoder(bit count or fields)", bpp.into());
            return Err(WINCODEC_ERR_UNSUPPORTEDOPERATION);
        }
    };
    // The colour table.
    let mut palette = None;
    if bpp <= 8 {
        let entries = if used != 0 && used <= 1 << bpp { used } else { 1 << bpp } as usize;
        let (at, size) = (14 + hsize, if core { 3 } else { 4 });
        let mut p = Vec::new();
        p.try_reserve_exact(entries).map_err(|_| E_OUTOFMEMORY)?;
        for i in 0..entries {
            let Some(c) = data.get(at + i * size..at + i * size + 3) else { break };
            p.push(0xff00_0000 | u32::from(c[2]) << 16 | u32::from(c[1]) << 8 | u32::from(c[0]));
        }
        palette = Some(p);
    }
    let file_row = row_bytes(w, bpp).next_multiple_of(4);
    let row = row_bytes(w, fmt.bpp);
    let size = row.checked_mul(h as usize).ok_or(E_OUTOFMEMORY)?;
    let mut pixels = zeroed(size).ok_or(E_OUTOFMEMORY)?;
    for y in 0..h as usize {
        let from = if top_down { y } else { h as usize - 1 - y };
        let at = bits_at + from * file_row;
        let src = data.get(at..at + row_bytes(w, bpp)).ok_or(bad)?;
        let dst = &mut pixels[y * row..(y + 1) * row];
        match (bpp, masks) {
            (32, Some((r, g, b))) => {
                for x in 0..w as usize {
                    let v = u32::from_le_bytes(src[4 * x..4 * x + 4].try_into().unwrap_or([0; 4]));
                    dst[4 * x..4 * x + 4].copy_from_slice(&[
                        field(v, b),
                        field(v, g),
                        field(v, r),
                        0,
                    ]);
                }
            }
            _ => dst.copy_from_slice(src),
        }
    }
    let dpi = if xppm != 0 && yppm != 0 {
        (f64::from(xppm) * 0.0254, f64::from(yppm) * 0.0254)
    } else {
        (96.0, 96.0)
    };
    Ok(Image { width: w, height: h, fmt, stride: row, data: pixels, palette, dpi })
}
