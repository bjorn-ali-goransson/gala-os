//! PNG decoding for GdipLoadImageFromStream, from the PNG specification
//! (W3C, Portable Network Graphics, Third Edition): the chunk layout,
//! IHDR, PLTE, tRNS, the zlib stream of the IDAT chunks (inflated by
//! miniz_oxide), the five filter types and the colour types at the bit
//! depths below. Pixels come out as straight 0xAARRGGBB.
//!
//! Decoded: colour types 0 (grey), 2 (RGB), 3 (palette), 4 (grey and
//! alpha), 6 (RGBA); bit depth 8, 16 (its high byte) and, for grey and
//! palette images, 1, 2 and 4; tRNS for types 0, 2 and 3. No gamma or
//! colour-space conversion: gAMA, cHRM, sRGB and iCCP are not applied
//! (an image whose gAMA is not the sRGB-like 1/2.2 is recorded,
//! `<dll>!png(gAMA not applied)`). Not decoded (recorded): interlaced
//! (Adam7) images. One source for two DLLs: gdiplus (PNG images) and
//! user32 (PNG icons, `#[path]`); each names its ledger lines
//! (`crate::PNG_GAMA`, `crate::PNG_INTERLACED`).

use alloc::vec::Vec;

pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

pub enum Error {
    NotPng,
    Corrupt,
    OutOfMemory,
    Unsupported(&'static [u8]),
}

const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// The largest image decoded (a guard against a header that asks for
/// more memory than any installer's picture).
const MAX_PIXELS: u64 = 1 << 26;

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

pub fn decode(data: &[u8]) -> Result<Decoded, Error> {
    if !data.starts_with(SIGNATURE) {
        return Err(Error::NotPng);
    }
    let mut at = SIGNATURE.len();
    let mut header = None;
    let mut palette: &[u8] = &[];
    let mut trns: &[u8] = &[];
    let mut idat = Vec::new();
    while at + 12 <= data.len() {
        let len = be32(&data[at..]) as usize;
        let kind = &data[at + 4..at + 8];
        let body = data.get(at + 8..at + 8 + len).ok_or(Error::Corrupt)?;
        match kind {
            b"IHDR" if body.len() >= 13 => header = Some(body),
            b"PLTE" => palette = body,
            b"tRNS" => trns = body,
            b"IDAT" => {
                idat.try_reserve(body.len()).map_err(|_| Error::OutOfMemory)?;
                idat.extend_from_slice(body);
            }
            b"gAMA" if body.len() >= 4 && !(45000..=46000).contains(&be32(body)) => {
                crate::record(crate::PNG_GAMA, be32(body).into());
            }
            b"IEND" => break,
            _ => {}
        }
        at += 12 + len;
    }
    let h = header.ok_or(Error::Corrupt)?;
    let (width, height) = (be32(h), be32(&h[4..]));
    let (depth, color, interlace) = (h[8], h[9], h[12]);
    if width == 0 || height == 0 || h[10] != 0 || h[11] != 0 {
        return Err(Error::Corrupt);
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(Error::OutOfMemory);
    }
    if interlace != 0 {
        return Err(Error::Unsupported(crate::PNG_INTERLACED));
    }
    let channels = match (color, depth) {
        (0, 1 | 2 | 4 | 8 | 16) | (3, 1 | 2 | 4 | 8) => 1,
        (2, 8 | 16) => 3,
        (4, 8 | 16) => 2,
        (6, 8 | 16) => 4,
        _ => return Err(Error::Corrupt),
    };
    let bits_per_pixel = channels * usize::from(depth);
    let bpp = bits_per_pixel.div_ceil(8);
    let row = (width as usize * bits_per_pixel).div_ceil(8);
    let size = (row + 1) * height as usize;
    let mut raw = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&idat, size)
        .map_err(|_| Error::Corrupt)?;
    if raw.len() < size {
        return Err(Error::Corrupt);
    }
    unfilter(&mut raw, row, bpp, height as usize)?;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(width as usize * height as usize).map_err(|_| Error::OutOfMemory)?;
    // tRNS: the transparent grey level or RGB colour (16-bit values), or
    // the palette entries' alphas.
    let key = |i: usize| trns.get(2 * i..2 * i + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    for y in 0..height as usize {
        let line = &raw[y * (row + 1) + 1..(y + 1) * (row + 1)];
        for x in 0..width as usize {
            // Sample k of pixel x (the high byte of a 16-bit one; small
            // depths scaled to 0-255) and its raw value (for tRNS).
            let sample = |k: usize| -> (u32, u16) {
                match depth {
                    8 => {
                        let v = line[x * channels + k];
                        (v.into(), v.into())
                    }
                    16 => {
                        let i = 2 * (x * channels + k);
                        (line[i].into(), u16::from_be_bytes([line[i], line[i + 1]]))
                    }
                    d => {
                        let bit = x * usize::from(d);
                        let shift = 8 - usize::from(d) - bit % 8;
                        let v = (line[bit / 8] >> shift) & ((1u8 << d) - 1);
                        let scaled = u32::from(v) * 255 / ((1u32 << d) - 1);
                        (if color == 3 { v.into() } else { scaled }, v.into())
                    }
                }
            };
            let argb = match color {
                0 => {
                    let (g, raw) = sample(0);
                    let a = if key(0) == Some(raw) { 0 } else { 255 };
                    a << 24 | g << 16 | g << 8 | g
                }
                2 => {
                    let ((r, rr), (g, rg), (b, rb)) = (sample(0), sample(1), sample(2));
                    let transparent =
                        key(0) == Some(rr) && key(1) == Some(rg) && key(2) == Some(rb);
                    let a = if transparent { 0 } else { 255 };
                    a << 24 | r << 16 | g << 8 | b
                }
                3 => {
                    let i = sample(0).1 as usize;
                    let p = palette.get(3 * i..3 * i + 3).ok_or(Error::Corrupt)?;
                    let a = trns.get(i).copied().unwrap_or(255);
                    u32::from(a) << 24
                        | u32::from(p[0]) << 16
                        | u32::from(p[1]) << 8
                        | u32::from(p[2])
                }
                4 => {
                    let (g, a) = (sample(0).0, sample(1).0);
                    a << 24 | g << 16 | g << 8 | g
                }
                _ => {
                    let (r, g, b, a) = (sample(0).0, sample(1).0, sample(2).0, sample(3).0);
                    a << 24 | r << 16 | g << 8 | b
                }
            };
            pixels.push(argb);
        }
    }
    Ok(Decoded { width, height, pixels })
}

/// Undoes the per-row filters (None, Sub, Up, Average, Paeth) in place;
/// each row is its filter byte and `row` bytes.
fn unfilter(raw: &mut [u8], row: usize, bpp: usize, height: usize) -> Result<(), Error> {
    for y in 0..height {
        let start = y * (row + 1);
        let filter = raw[start];
        for i in 0..row {
            let at = start + 1 + i;
            let a = if i >= bpp { raw[at - bpp] } else { 0 };
            let b = if y > 0 { raw[at - row - 1] } else { 0 };
            let c = if y > 0 && i >= bpp { raw[at - row - 1 - bpp] } else { 0 };
            let add = match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                4 => paeth(a, b, c),
                _ => return Err(Error::Corrupt),
            };
            raw[at] = raw[at].wrapping_add(add);
        }
    }
    Ok(())
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = i16::from(a) + i16::from(b) - i16::from(c);
    let (pa, pb, pc) =
        ((p - i16::from(a)).abs(), (p - i16::from(b)).abs(), (p - i16::from(c)).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}
