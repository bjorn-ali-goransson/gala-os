//! Pixel memory a DC draws on: a window's client area (from user32), a
//! device-dependent bitmap (32 bits per pixel, top-down, whatever its
//! logical depth) or a DIB section in the program's own format (1, 24 or
//! 32 bits, top-down or bottom-up; 1-bit pixels index a two-colour
//! palette, bit 7 the leftmost). Colours are 0x00RRGGBB, which is a 32-bit
//! DIB's memory order (blue, green, red, unused) read as a u32.

#[derive(Clone, Copy)]
pub struct Surface {
    pub bits: *mut u8,
    pub width: i32,
    pub height: i32,
    /// Bytes per row.
    pub stride: i32,
    /// 1, 24 or 32.
    pub bpp: u16,
    /// A 1-bit surface's colours (0x00RRGGBB).
    pub palette: [u32; 2],
    /// Row 0 in memory is the bottom line.
    pub bottom_up: bool,
    /// A monochrome bitmap: what is written becomes black or white.
    pub mono: bool,
}

impl Surface {
    pub const NONE: Surface = Surface {
        bits: core::ptr::null_mut(),
        width: 0,
        height: 0,
        stride: 0,
        bpp: 32,
        palette: [0, 0x00ff_ffff],
        bottom_up: false,
        mono: false,
    };

    pub fn contains(&self, x: i32, y: i32) -> bool {
        !self.bits.is_null() && x >= 0 && y >= 0 && x < self.width && y < self.height
    }

    fn at(&self, x: i32, y: i32) -> *mut u8 {
        let row = if self.bottom_up { self.height - 1 - y } else { y };
        // SAFETY: callers check `contains`; the offset is inside the
        // surface's rows.
        unsafe {
            self.bits.add(
                (row as isize * self.stride as isize + x as isize * (self.bpp as isize / 8))
                    as usize,
            )
        }
    }

    /// A 1-bit surface's byte holding (x, y) and the pixel's bit in it.
    fn bit(&self, x: i32, y: i32) -> (*mut u8, u8) {
        let row = if self.bottom_up { self.height - 1 - y } else { y };
        // SAFETY: callers check `contains`; the byte is inside the row.
        let p = unsafe { self.bits.add(row as usize * self.stride as usize + x as usize / 8) };
        (p, 0x80 >> (x % 8))
    }

    /// The pixel at (x, y), if inside.
    pub fn get(&self, x: i32, y: i32) -> Option<u32> {
        if !self.contains(x, y) {
            return None;
        }
        if self.bpp == 1 {
            let (p, bit) = self.bit(x, y);
            // SAFETY: inside the surface.
            return Some(self.palette[usize::from(unsafe { *p } & bit != 0)]);
        }
        let p = self.at(x, y);
        // SAFETY: inside the surface.
        Some(unsafe {
            if self.bpp == 32 {
                p.cast::<u32>().read_unaligned() & 0x00ff_ffff
            } else {
                u32::from(*p) | u32::from(*p.add(1)) << 8 | u32::from(*p.add(2)) << 16
            }
        })
    }

    /// The pixel with its fourth byte (a 32-bit surface's alpha; 0 for 24
    /// bits), if inside.
    pub fn get_raw(&self, x: i32, y: i32) -> Option<u32> {
        if !self.contains(x, y) {
            return None;
        }
        if self.bpp != 32 {
            return self.get(x, y);
        }
        // SAFETY: inside the surface.
        Some(unsafe { self.at(x, y).cast::<u32>().read_unaligned() })
    }

    /// Writes (x, y) with its fourth byte (32-bit surfaces).
    pub fn put_raw(&self, x: i32, y: i32, c: u32) {
        if self.contains(x, y) && self.bpp == 32 && !self.mono {
            // SAFETY: inside the surface.
            unsafe { self.at(x, y).cast::<u32>().write_unaligned(c) };
        } else {
            self.put(x, y, c);
        }
    }

    /// Writes (x, y) if inside.
    pub fn put(&self, x: i32, y: i32, c: u32) {
        if !self.contains(x, y) {
            return;
        }
        let c = if self.mono { mono(c) } else { c & 0x00ff_ffff };
        if self.bpp == 1 {
            // The nearer palette entry.
            let far = |e: u32| {
                (0..3)
                    .map(|k| {
                        let d = i32::from((c >> (8 * k)) as u8) - i32::from((e >> (8 * k)) as u8);
                        d * d
                    })
                    .sum::<i32>()
            };
            let (p, bit) = self.bit(x, y);
            // SAFETY: inside the surface.
            unsafe {
                if far(self.palette[1]) < far(self.palette[0]) {
                    *p |= bit;
                } else {
                    *p &= !bit;
                }
            }
            return;
        }
        let p = self.at(x, y);
        // SAFETY: inside the surface.
        unsafe {
            if self.bpp == 32 {
                p.cast::<u32>().write_unaligned(c);
            } else {
                *p = c as u8;
                *p.add(1) = (c >> 8) as u8;
                *p.add(2) = (c >> 16) as u8;
            }
        }
    }
}

/// A colour on a monochrome bitmap: white if it is the lighter half.
pub fn mono(c: u32) -> u32 {
    let (r, g, b) = ((c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff);
    if r * 30 + g * 59 + b * 11 >= 128 * 100 { 0x00ff_ffff } else { 0 }
}

pub fn blend(bg: u32, fg: u32, t: u8) -> u32 {
    let t = u32::from(t);
    let mix = |shift: u32| {
        let (a, b) = ((bg >> shift) & 0xff, (fg >> shift) & 0xff);
        ((a * (255 - t) + b * t) / 255) << shift
    };
    mix(16) | mix(8) | mix(0)
}
