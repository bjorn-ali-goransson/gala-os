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
