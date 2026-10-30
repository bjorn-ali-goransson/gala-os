//! Safe wrapper around the bootloader-provided (UEFI GOP) framebuffer.

use core::ptr::NonNull;

use super::limine::{FRAMEBUFFER_RGB, RawFramebuffer};

/// A linear 32-bpp RGB framebuffer.
pub struct Framebuffer {
    base: NonNull<u8>,
    width: u64,
    height: u64,
    pitch: u64,
    bpp: u16,
    channels: [(u8, u8); 3], // (mask size, shift) for red, green, blue
}

// SAFETY: the value owns the framebuffer memory (see `from_raw`); which
// thread or CPU draws through it does not matter.
unsafe impl Send for Framebuffer {}

impl Framebuffer {
    /// Returns `None` for framebuffers this driver cannot draw on.
    ///
    /// # Safety
    /// `raw.address` must map `raw.pitch * raw.height` bytes of framebuffer
    /// memory, valid for `'static`, that no other code accesses.
    pub(super) unsafe fn from_raw(raw: &RawFramebuffer) -> Option<Self> {
        let channels = [
            (raw.red_mask_size, raw.red_mask_shift),
            (raw.green_mask_size, raw.green_mask_shift),
            (raw.blue_mask_size, raw.blue_mask_shift),
        ];
        let usable = raw.memory_model == FRAMEBUFFER_RGB
            && raw.bpp == 32
            && raw.width.checked_mul(4).is_some_and(|row| row <= raw.pitch)
            && channels
                .iter()
                .all(|&(size, shift)| size <= 8 && u32::from(size) + u32::from(shift) <= 32);
        if !usable {
            return None;
        }
        Some(Self {
            base: NonNull::new(raw.address)?,
            width: raw.width,
            height: raw.height,
            pitch: raw.pitch,
            bpp: raw.bpp,
            channels,
        })
    }

    pub fn width(&self) -> u64 {
        self.width
    }

    pub fn height(&self) -> u64 {
        self.height
    }

    pub fn pitch(&self) -> u64 {
        self.pitch
    }

    pub fn bpp(&self) -> u16 {
        self.bpp
    }

    /// Converts `0x00RRGGBB` to this framebuffer's pixel format.
    fn encode(&self, rgb: u32) -> u32 {
        let components = [(rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff];
        components
            .iter()
            .zip(self.channels)
            .map(|(&c, (size, shift))| (c >> (8 - size)) << shift)
            .fold(0, |pixel, c| pixel | c)
    }

    /// Writes an encoded pixel; coordinates outside the screen are ignored.
    fn put(&self, x: u64, y: u64, pixel: u32) {
        if x >= self.width || y >= self.height {
            return;
        }
        // SAFETY: as in `fill`: inside the `pitch * height` bytes guaranteed
        // by `from_raw`, 4-byte aligned.
        unsafe {
            let p = self.base.as_ptr().add((y * self.pitch + x * 4) as usize).cast::<u32>();
            p.write_volatile(pixel);
        }
    }

    /// Fills a rectangle (clipped to the screen) with `0x00RRGGBB`.
    pub fn fill_rect(&self, x: u64, y: u64, w: u64, h: u64, rgb: u32) {
        let pixel = self.encode(rgb);
        for yy in y..(y + h).min(self.height) {
            for xx in x..(x + w).min(self.width) {
                self.put(xx, yy, pixel);
            }
        }
    }

    /// Draws a grayscale glyph (`rows[y][x]` = coverage 0..=255) at (x, y),
    /// blending `fg` over `bg` (both `0x00RRGGBB`).
    pub fn draw_glyph(&self, x: u64, y: u64, rows: &[&[u8]], fg: u32, bg: u32) {
        let mix = |a: u32, b: u32, t: u32, shift: u32| {
            let (a, b) = ((a >> shift) & 0xff, (b >> shift) & 0xff);
            ((a * (255 - t) + b * t) / 255) << shift
        };
        for (dy, row) in rows.iter().enumerate() {
            for (dx, &coverage) in row.iter().enumerate() {
                let t = u32::from(coverage);
                let rgb = mix(bg, fg, t, 16) | mix(bg, fg, t, 8) | mix(bg, fg, t, 0);
                self.put(x + dx as u64, y + dy as u64, self.encode(rgb));
            }
        }
    }

    /// Fills the whole screen with `0x00RRGGBB`.
    pub fn fill(&self, rgb: u32) {
        let pixel = self.encode(rgb);
        for y in 0..self.height {
            let row = (y * self.pitch) as usize;
            for x in 0..self.width as usize {
                // SAFETY: `x < width` and `width * 4 <= pitch`, so the 4-byte
                // pixel lies inside the `pitch * height` bytes guaranteed by
                // `from_raw`; GOP framebuffers are 4-byte aligned.
                unsafe {
                    let p = self.base.as_ptr().add(row + x * 4).cast::<u32>();
                    p.write_volatile(pixel);
                }
            }
        }
    }
}
