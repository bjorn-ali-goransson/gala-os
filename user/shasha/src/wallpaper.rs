//! The wallpaper (ROADMAP SH step 1, part 9): the picture the user's
//! "Wallpaper" value names, decoded and scaled to cover the screen, which
//! shasha's desktop window (Progman, the shell's window) shows. gala's
//! wallpapers are QOI pictures; another kind is not shown.

use alloc::vec::Vec;

/// A picture: `w` x `h` 0xAARRGGBB pixels (straight alpha), rows top
/// first.
#[derive(Debug, PartialEq, Eq)]
pub struct Picture {
    pub w: i32,
    pub h: i32,
    pub pixels: Vec<u32>,
}

/// Why a picture could not be shown.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Not a QOI picture, or cut short.
    Malformed,
    /// Wider or higher than `MAX_SIDE`.
    TooLarge,
    /// Memory ran out.
    NoMemory,
}

/// The largest side a wallpaper may have.
pub const MAX_SIDE: u32 = 8192;

/// Decodes a QOI picture (written from the specification at
/// qoiformat.org: a 14-byte header, "qoif", width and height big endian,
/// channels and colour space; then chunks: RGB, RGBA, INDEX into the 64
/// recent colours, DIFF, LUMA and RUN, until every pixel is had).
pub fn decode_qoi(data: &[u8]) -> Result<Picture, Refused> {
    if data.len() < 14 || &data[..4] != b"qoif" {
        return Err(Refused::Malformed);
    }
    let w = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let h = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
    if w == 0 || h == 0 {
        return Err(Refused::Malformed);
    }
    if w > MAX_SIDE || h > MAX_SIDE {
        return Err(Refused::TooLarge);
    }
    let n = w as usize * h as usize;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(n).map_err(|_| Refused::NoMemory)?;
    let mut index = [[0u8; 4]; 64];
    let mut px = [0u8, 0, 0, 255];
    let mut p = 14;
    let byte = |p: usize| data.get(p).copied().ok_or(Refused::Malformed);
    while pixels.len() < n {
        let b = byte(p)?;
        p += 1;
        let mut run = 1;
        match b {
            0xfe => {
                px[..3].copy_from_slice(data.get(p..p + 3).ok_or(Refused::Malformed)?);
                p += 3;
            }
            0xff => {
                px.copy_from_slice(data.get(p..p + 4).ok_or(Refused::Malformed)?);
                p += 4;
            }
            _ => match b >> 6 {
                0 => px = index[usize::from(b & 63)],
                1 => {
                    px[0] = px[0].wrapping_add((b >> 4) & 3).wrapping_sub(2);
                    px[1] = px[1].wrapping_add((b >> 2) & 3).wrapping_sub(2);
                    px[2] = px[2].wrapping_add(b & 3).wrapping_sub(2);
                }
                2 => {
                    let b2 = byte(p)?;
                    p += 1;
                    let dg = (b & 0x3f).wrapping_sub(32);
                    px[0] = px[0].wrapping_add(dg).wrapping_sub(8).wrapping_add(b2 >> 4);
                    px[1] = px[1].wrapping_add(dg);
                    px[2] = px[2].wrapping_add(dg).wrapping_sub(8).wrapping_add(b2 & 15);
                }
                _ => run = usize::from(b & 63) + 1,
            },
        }
        let hash = (usize::from(px[0]) * 3
            + usize::from(px[1]) * 5
            + usize::from(px[2]) * 7
            + usize::from(px[3]) * 11)
            % 64;
        index[hash] = px;
        let c = u32::from(px[3]) << 24
            | u32::from(px[0]) << 16
            | u32::from(px[1]) << 8
            | u32::from(px[2]);
        for _ in 0..run.min(n - pixels.len()) {
            pixels.push(c);
        }
    }
    Ok(Picture { w: w as i32, h: h as i32, pixels })
}

/// `pic` scaled to cover a `w` x `h` screen (the larger of the two
/// scales, the overflow cut evenly from both sides; nearest pixel),
/// opaque.
pub fn cover(pic: &Picture, w: i32, h: i32) -> Result<Picture, Refused> {
    if w <= 0 || h <= 0 || pic.w <= 0 || pic.h <= 0 {
        return Err(Refused::Malformed);
    }
    let (w64, h64, pw, ph) = (i64::from(w), i64::from(h), i64::from(pic.w), i64::from(pic.h));
    let mut pixels = Vec::new();
    pixels.try_reserve_exact((w64 * h64) as usize).map_err(|_| Refused::NoMemory)?;
    // The scale num / den: w / pw when that covers the height, else h / ph.
    let (num, den) = if w64 * ph >= h64 * pw { (w64, pw) } else { (h64, ph) };
    let (ox, oy) = ((pw * num / den - w64) / 2, (ph * num / den - h64) / 2);
    for y in 0..h64 {
        let sy = ((y + oy) * den / num).clamp(0, ph - 1);
        for x in 0..w64 {
            let sx = ((x + ox) * den / num).clamp(0, pw - 1);
            let c = pic.pixels.get((sy * pw + sx) as usize).copied().unwrap_or(0);
            pixels.push(c | 0xff00_0000);
        }
    }
    Ok(Picture { w, h, pixels })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A QOI picture of `w` x `h` from its chunks, with the end marker.
    fn qoi(w: u32, h: u32, chunks: &[u8]) -> Vec<u8> {
        let mut d = b"qoif".to_vec();
        d.extend_from_slice(&w.to_be_bytes());
        d.extend_from_slice(&h.to_be_bytes());
        d.extend_from_slice(&[4, 0]);
        d.extend_from_slice(chunks);
        d.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
        d
    }

    #[test]
    fn each_chunk_decodes_as_the_specification_says() {
        // RGB red; RUN of 2 (red again); DIFF +1,-1,+0 (from red:
        // 0x01, 0xff, 0x00 -> r 0, g 255, b 0 wrapping); RGBA; INDEX of
        // red (hash (255*3 + 255*11) % 64 = 50); LUMA dg +1, dr-dg 0,
        // db-dg 0 from red: (0, 0, 1) wrapping.
        let chunks = [
            0xfe,
            255,
            0,
            0,                              // RGB
            0xc0 | 1,                       // RUN 2
            0x40 | (3 << 4) | (1 << 2) | 2, // DIFF +1, -1, 0
            0xff,
            1,
            2,
            3,
            128, // RGBA
            50,  // INDEX red
            0x80 | (32 + 1),
            0x88, // LUMA dg +1, dr-dg 0, db-dg 0
        ];
        let p = decode_qoi(&qoi(7, 1, &chunks)).unwrap();
        assert_eq!(
            p.pixels,
            vec![
                0xffff_0000,
                0xffff_0000,
                0xffff_0000,
                0xff00_ff00,
                0x8001_0203,
                0xffff_0000,
                0xff00_0101
            ]
        );
    }

    #[test]
    fn a_bad_picture_is_refused() {
        assert_eq!(decode_qoi(b"PNG...."), Err(Refused::Malformed));
        assert_eq!(decode_qoi(&qoi(0, 1, &[])), Err(Refused::Malformed));
        assert_eq!(decode_qoi(&qoi(9000, 1, &[])), Err(Refused::TooLarge));
        // Cut short: twenty pixels promised, one given.
        let mut d = qoi(20, 1, &[0xfe, 1, 2, 3]);
        d.truncate(d.len() - 8);
        assert_eq!(decode_qoi(&d), Err(Refused::Malformed));
    }

    #[test]
    fn cover_fills_the_screen_cutting_the_overflow_evenly() {
        // 4 x 2 columns 0..3 onto 2 x 2: scale 1 by height, cut one
        // column from each side.
        let pic = Picture { w: 4, h: 2, pixels: vec![0, 1, 2, 3, 0, 1, 2, 3] };
        let c = cover(&pic, 2, 2).unwrap();
        assert_eq!(c.pixels, vec![0xff00_0001, 0xff00_0002, 0xff00_0001, 0xff00_0002]);
        // 1 x 1 onto 3 x 2: every pixel the one, opaque.
        let one = Picture { w: 1, h: 1, pixels: vec![0x0012_3456] };
        assert_eq!(cover(&one, 3, 2).unwrap().pixels, vec![0xff12_3456; 6]);
        // Doubled: each pixel twice across and down.
        let two = Picture { w: 2, h: 1, pixels: vec![5, 6] };
        assert_eq!(
            cover(&two, 4, 2).unwrap().pixels,
            vec![
                0xff00_0005,
                0xff00_0005,
                0xff00_0006,
                0xff00_0006,
                0xff00_0005,
                0xff00_0005,
                0xff00_0006,
                0xff00_0006
            ]
        );
        assert_eq!(cover(&one, 0, 2), Err(Refused::Malformed));
    }
}
