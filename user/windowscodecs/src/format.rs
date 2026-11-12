//! WIC's pixel formats (the GUID_WICPixelFormat* of wincodec.h) that
//! this DLL knows, and their conversion through straight 0xAARRGGBB.
//!
//! Read (a source in it can be converted, scaled and copied): the
//! indexed formats (1, 2, 4, 8 bpp, through the source's palette),
//! BlackWhite (1 is white), 2/4/8 and 16 bpp grey, 16 bpp BGR555 and
//! BGR565, 24 bpp BGR and RGB, 32 bpp BGR, BGRA, PBGRA, RGB, RGBA and
//! PRGBA, 48 bpp RGB, 64 bpp RGBA and PRGBA. Written (a converter can
//! produce it): the 16, 24 and 32 bpp colour formats, 48 bpp RGB and
//! 64 bpp RGBA. Converting to indexed formats (a palette to choose) and
//! to grey (Windows weighs the channels in linear light) is not done
//! (recorded by the converter).
//!
//! Premultiplying rounds to nearest (c * a / 255), the values of Wine's
//! converter test BGRA -> PBGRA (run on Windows); a 16-bit channel
//! becomes 8 bits rounded (v * 255 / 65535) and an 8-bit one 16 bits
//! as v * 257; 5- and 6-bit channels are widened by repeating their high
//! bits. The fourth byte of 32bppBGR and 32bppRGB is written as 0xFF.

use crate::{Guid, get_bits, guid};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Indexed,
    BlackWhite,
    Gray,
    Gray16,
    Bgr555,
    Bgr565,
    Bgr24,
    Rgb24,
    Bgr32,
    Bgra32,
    Pbgra32,
    Rgb32,
    Rgba32,
    Prgba32,
    Rgb48,
    Rgba64,
    Prgba64,
}

pub struct Fmt {
    pub guid: Guid,
    pub bpp: u32,
    pub kind: Kind,
}

/// GUID_WICPixelFormat* with the shared prefix, ending in `last`.
const fn base(last: u8) -> Guid {
    guid(0x6fdd_c324, 0x4e03, 0x4bfe, [0xb1, 0x85, 0x3d, 0x77, 0x76, 0x8d, 0xc9, last])
}

pub const DONT_CARE: Guid = base(0x00);
pub const BLACK_WHITE: Guid = base(0x05);
pub const INDEXED_1: Guid = base(0x01);
pub const INDEXED_2: Guid = base(0x02);
pub const INDEXED_4: Guid = base(0x03);
pub const INDEXED_8: Guid = base(0x04);
pub const GRAY_2: Guid = base(0x06);
pub const GRAY_4: Guid = base(0x07);
pub const GRAY_8: Guid = base(0x08);
pub const GRAY_16: Guid = base(0x0b);
pub const BGR555: Guid = base(0x09);
pub const BGR565: Guid = base(0x0a);
pub const BGR24: Guid = base(0x0c);
pub const RGB24: Guid = base(0x0d);
pub const BGR32: Guid = base(0x0e);
pub const BGRA32: Guid = base(0x0f);
pub const PBGRA32: Guid = base(0x10);
pub const RGB48: Guid = base(0x15);
pub const RGBA64: Guid = base(0x16);
pub const PRGBA64: Guid = base(0x17);
pub const RGB32: Guid =
    guid(0xd98c_6b95, 0x3efe, 0x47d6, [0xbb, 0x25, 0xeb, 0x17, 0x48, 0xab, 0x0c, 0xf1]);
pub const RGBA32: Guid =
    guid(0xf5c7_ad2d, 0x6a8d, 0x43dd, [0xa7, 0xa8, 0xa2, 0x99, 0x35, 0x26, 0x1a, 0xe9]);
pub const PRGBA32: Guid =
    guid(0x3cc4_a650, 0xa527, 0x4d37, [0xa9, 0x16, 0x31, 0x42, 0xc7, 0xeb, 0xed, 0xba]);

pub static FORMATS: [Fmt; 23] = [
    Fmt { guid: INDEXED_1, bpp: 1, kind: Kind::Indexed },
    Fmt { guid: INDEXED_2, bpp: 2, kind: Kind::Indexed },
    Fmt { guid: INDEXED_4, bpp: 4, kind: Kind::Indexed },
    Fmt { guid: INDEXED_8, bpp: 8, kind: Kind::Indexed },
    Fmt { guid: BLACK_WHITE, bpp: 1, kind: Kind::BlackWhite },
    Fmt { guid: GRAY_2, bpp: 2, kind: Kind::Gray },
    Fmt { guid: GRAY_4, bpp: 4, kind: Kind::Gray },
    Fmt { guid: GRAY_8, bpp: 8, kind: Kind::Gray },
    Fmt { guid: GRAY_16, bpp: 16, kind: Kind::Gray16 },
    Fmt { guid: BGR555, bpp: 16, kind: Kind::Bgr555 },
    Fmt { guid: BGR565, bpp: 16, kind: Kind::Bgr565 },
    Fmt { guid: BGR24, bpp: 24, kind: Kind::Bgr24 },
    Fmt { guid: RGB24, bpp: 24, kind: Kind::Rgb24 },
    Fmt { guid: BGR32, bpp: 32, kind: Kind::Bgr32 },
    Fmt { guid: BGRA32, bpp: 32, kind: Kind::Bgra32 },
    Fmt { guid: PBGRA32, bpp: 32, kind: Kind::Pbgra32 },
    Fmt { guid: RGB32, bpp: 32, kind: Kind::Rgb32 },
    Fmt { guid: RGBA32, bpp: 32, kind: Kind::Rgba32 },
    Fmt { guid: PRGBA32, bpp: 32, kind: Kind::Prgba32 },
    Fmt { guid: RGB48, bpp: 48, kind: Kind::Rgb48 },
    Fmt { guid: RGBA64, bpp: 64, kind: Kind::Rgba64 },
    Fmt { guid: PRGBA64, bpp: 64, kind: Kind::Prgba64 },
    // Last: GUID_WICPixelFormatDontCare is no format of its own.
    Fmt { guid: DONT_CARE, bpp: 0, kind: Kind::Indexed },
];

/// The format `g` names (None for one this DLL does not know).
pub fn find(g: &Guid) -> Option<&'static Fmt> {
    FORMATS.iter().find(|f| f.guid == *g && f.bpp != 0)
}

/// Whether a converter can produce `f`.
pub fn writable(f: &Fmt) -> bool {
    !matches!(f.kind, Kind::Indexed | Kind::BlackWhite | Kind::Gray | Kind::Gray16)
}

/// Whether `f` holds one byte per channel (the scaler interpolates such
/// formats channel by channel).
pub fn byte_channels(f: &Fmt) -> bool {
    matches!(
        f.kind,
        Kind::Bgr24
            | Kind::Rgb24
            | Kind::Bgr32
            | Kind::Bgra32
            | Kind::Pbgra32
            | Kind::Rgb32
            | Kind::Rgba32
            | Kind::Prgba32
    ) || (f.kind == Kind::Gray && f.bpp == 8)
}

fn argb(a: u32, r: u32, g: u32, b: u32) -> u32 {
    a << 24 | r << 16 | g << 8 | b
}

fn le16(b: &[u8], i: usize) -> u32 {
    u32::from(u16::from_le_bytes([b[i], b[i + 1]]))
}

/// A 16-bit channel as 8 bits, rounded.
fn to8(v: u32) -> u32 {
    (v * 255 + 32767) / 65535
}

/// c * a / 255, rounded.
pub fn premultiply(c: u32, a: u32) -> u32 {
    (c * a + 127) / 255
}

/// c * 255 / a, rounded and clamped (0 when a is 0).
pub fn unpremultiply(c: u32, a: u32) -> u32 {
    (c * 255 + a / 2).checked_div(a).map_or(0, |v| v.min(255))
}

/// Straight ARGB from premultiplied channels.
fn straight(a: u32, r: u32, g: u32, b: u32) -> u32 {
    argb(a, unpremultiply(r, a), unpremultiply(g, a), unpremultiply(b, a))
}

/// Pixel `x` of `line` (in format `f`) as straight 0xAARRGGBB; `palette`
/// serves the indexed formats (an index past it is opaque black).
pub fn read(f: &Fmt, line: &[u8], x: usize, palette: &[u32]) -> u32 {
    match f.kind {
        Kind::Indexed => {
            let i = get_bits(line, x, f.bpp) as usize;
            palette.get(i).copied().unwrap_or(0xff00_0000)
        }
        Kind::BlackWhite => {
            if get_bits(line, x, 1) != 0 {
                0xffff_ffff
            } else {
                0xff00_0000
            }
        }
        Kind::Gray => {
            let v = get_bits(line, x, f.bpp) * 255 / ((1 << f.bpp) - 1);
            argb(255, v, v, v)
        }
        Kind::Gray16 => {
            let v = to8(le16(line, 2 * x));
            argb(255, v, v, v)
        }
        Kind::Bgr555 => {
            let v = le16(line, 2 * x);
            let (r, g, b) = ((v >> 10) & 31, (v >> 5) & 31, v & 31);
            argb(255, r << 3 | r >> 2, g << 3 | g >> 2, b << 3 | b >> 2)
        }
        Kind::Bgr565 => {
            let v = le16(line, 2 * x);
            let (r, g, b) = ((v >> 11) & 31, (v >> 5) & 63, v & 31);
            argb(255, r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2)
        }
        Kind::Bgr24 => {
            let p = &line[3 * x..];
            argb(255, p[2].into(), p[1].into(), p[0].into())
        }
        Kind::Rgb24 => {
            let p = &line[3 * x..];
            argb(255, p[0].into(), p[1].into(), p[2].into())
        }
        Kind::Bgr32 | Kind::Bgra32 | Kind::Pbgra32 => {
            let p = &line[4 * x..];
            let (b, g, r, a) = (p[0].into(), p[1].into(), p[2].into(), p[3].into());
            match f.kind {
                Kind::Bgr32 => argb(255, r, g, b),
                Kind::Bgra32 => argb(a, r, g, b),
                _ => straight(a, r, g, b),
            }
        }
        Kind::Rgb32 | Kind::Rgba32 | Kind::Prgba32 => {
            let p = &line[4 * x..];
            let (r, g, b, a) = (p[0].into(), p[1].into(), p[2].into(), p[3].into());
            match f.kind {
                Kind::Rgb32 => argb(255, r, g, b),
                Kind::Rgba32 => argb(a, r, g, b),
                _ => straight(a, r, g, b),
            }
        }
        Kind::Rgb48 => {
            let i = 6 * x;
            argb(255, to8(le16(line, i)), to8(le16(line, i + 2)), to8(le16(line, i + 4)))
        }
        Kind::Rgba64 | Kind::Prgba64 => {
            let i = 8 * x;
            let (r, g, b, a) = (
                to8(le16(line, i)),
                to8(le16(line, i + 2)),
                to8(le16(line, i + 4)),
                to8(le16(line, i + 6)),
            );
            if f.kind == Kind::Rgba64 { argb(a, r, g, b) } else { straight(a, r, g, b) }
        }
    }
}

/// Writes straight 0xAARRGGBB `v` as pixel `x` of `line` in format `f`
/// (one of the `writable` formats).
pub fn write(f: &Fmt, line: &mut [u8], x: usize, v: u32) {
    let (a, r, g, b) = (v >> 24, (v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff);
    let put16 = |line: &mut [u8], i: usize, w: u32| {
        line[i..i + 2].copy_from_slice(&(w as u16).to_le_bytes());
    };
    match f.kind {
        Kind::Bgr555 => put16(line, 2 * x, (r >> 3) << 10 | (g >> 3) << 5 | b >> 3),
        Kind::Bgr565 => put16(line, 2 * x, (r >> 3) << 11 | (g >> 2) << 5 | b >> 3),
        Kind::Bgr24 => line[3 * x..3 * x + 3].copy_from_slice(&[b as u8, g as u8, r as u8]),
        Kind::Rgb24 => line[3 * x..3 * x + 3].copy_from_slice(&[r as u8, g as u8, b as u8]),
        Kind::Bgr32 => line[4 * x..4 * x + 4].copy_from_slice(&[b as u8, g as u8, r as u8, 255]),
        Kind::Bgra32 => {
            line[4 * x..4 * x + 4].copy_from_slice(&[b as u8, g as u8, r as u8, a as u8])
        }
        Kind::Pbgra32 => line[4 * x..4 * x + 4].copy_from_slice(&[
            premultiply(b, a) as u8,
            premultiply(g, a) as u8,
            premultiply(r, a) as u8,
            a as u8,
        ]),
        Kind::Rgb32 => line[4 * x..4 * x + 4].copy_from_slice(&[r as u8, g as u8, b as u8, 255]),
        Kind::Rgba32 => {
            line[4 * x..4 * x + 4].copy_from_slice(&[r as u8, g as u8, b as u8, a as u8])
        }
        Kind::Prgba32 => line[4 * x..4 * x + 4].copy_from_slice(&[
            premultiply(r, a) as u8,
            premultiply(g, a) as u8,
            premultiply(b, a) as u8,
            a as u8,
        ]),
        Kind::Rgb48 => {
            for (k, c) in [r, g, b].into_iter().enumerate() {
                put16(line, 6 * x + 2 * k, c * 257);
            }
        }
        Kind::Rgba64 => {
            for (k, c) in [r, g, b, a].into_iter().enumerate() {
                put16(line, 8 * x + 2 * k, c * 257);
            }
        }
        Kind::Prgba64 => {
            for (k, c) in [r, g, b].into_iter().enumerate() {
                put16(line, 8 * x + 2 * k, c * a * 257 / 255);
            }
            put16(line, 8 * x + 6, a * 257);
        }
        Kind::Indexed | Kind::BlackWhite | Kind::Gray | Kind::Gray16 => {}
    }
}
