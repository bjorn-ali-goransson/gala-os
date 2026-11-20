//! The monitor's EDID: what gala's panel says about itself, built from
//! the screen's size and written where Windows keeps a monitor's EDID --
//! a REG_BINARY value under the monitor device's key in the registry,
//! which setupapi hands to a program that opens that key (DIREG_DEV).
//! DXVK's `getMonitorEdid` reads it there and Word draws through DXVK,
//! so without it the display has no metadata (ROADMAP OT3b).
//!
//! The block is the 128-byte base block of VESA's E-EDID, written from
//! the standard (CLEAN_ROOM): the header and the ids, the chromaticity
//! of sRGB, one detailed timing for the screen's own size with CVT
//! reduced blanking, and the panel's name. There are no extension
//! blocks: gala's display has no CEA modes, no HDR metadata and no
//! second timing to offer, and a base block alone is a whole EDID.
//!
//! gala's panel is 96 dots per inch, which is what its size in
//! millimetres below says and what the rest of the display assumes.

use crate::nt::registry::{self, Registry};
use crate::nt::status::NtStatus;
use crate::nt::{platform, scm};

/// The base block, which is the whole of gala's EDID.
pub const BLOCK: usize = 128;

/// Where the monitor device keeps it, and under which name. The device
/// is the one `registry.seed` describes and `user/user32/src/ccd.rs`
/// names: one monitor, hardware id GALA0001, instance 1&gala&0&UID1.
const KEY: &str = r"\Registry\Machine\SYSTEM\CurrentControlSet\Enum\DISPLAY\GALA0001\1&gala&0&UID1\Device Parameters";
const VALUE: &str = "EDID";
const REG_BINARY: u32 = 3;

/// The refresh rate the rest of the display reports (`ccd.rs`).
const REFRESH_HZ: u32 = 60;

/// The panel's name, which Windows takes from the EDID and which the CCD
/// gives as the target's friendly name.
const NAME: &[u8] = b"gala display";

/// CVT reduced blanking, version 1, as the standard fixes it: 160 pixels
/// of horizontal blanking, of which 48 are the front porch and 32 the
/// sync pulse, and enough blank lines for 460 us of vertical blanking,
/// never fewer than the 3 front and 6 back the standard leaves.
const H_BLANK: u32 = 160;
const H_FRONT: u32 = 48;
const H_SYNC: u32 = 32;
const V_FRONT: u32 = 3;
const V_BACK: u32 = 6;
const V_BLANK_NS: u32 = 460_000;

/// A monitor's EDID is its panel's, so the screen gala has: the frame
/// buffer's size, or the size a desktop nothing shows is laid out at.
fn panel() -> (u32, u32) {
    platform::screen_size().unwrap_or(super::DEFAULT_SIZE)
}

/// The three letters of a manufacturer id, five bits each with A as 1,
/// in the two bytes EDID keeps them in (the most significant first, the
/// top bit zero).
fn manufacturer(id: &[u8; 3]) -> [u8; 2] {
    let mut bits = 0u16;
    for &c in id {
        bits = (bits << 5) | u16::from(c.to_ascii_uppercase() - b'A' + 1);
    }
    bits.to_be_bytes()
}

/// The vertical sync pulse CVT gives a shape, in lines; a size with no
/// shape of its own gets the widest the standard defines.
fn v_sync_lines(width: u32, height: u32) -> u32 {
    if width * 3 == height * 4 {
        4
    } else if width * 9 == height * 16 {
        5
    } else if width * 10 == height * 16 {
        6
    } else if width * 4 == height * 5 || width * 9 == height * 15 {
        7
    } else {
        10
    }
}

/// A size in pixels as millimetres at 96 dots per inch.
fn millimetres(pixels: u32) -> u32 {
    (pixels * 254 + 480) / 960
}

/// The 18-byte detailed timing descriptor for a mode: the one timing
/// gala's panel prefers, and the only one it has.
fn detailed_timing(width: u32, height: u32, hz: u32) -> [u8; 18] {
    let (h_front, h_sync, v_front) = (H_FRONT, H_SYNC, V_FRONT);
    let v_sync = v_sync_lines(width, height);
    let h_total = width + H_BLANK;
    // The blank lines 460 us comes to, from the line time the rest of
    // the frame leaves.
    let line_ns = (1_000_000_000 / hz.max(1)).saturating_sub(V_BLANK_NS) / height.max(1);
    let v_blank = V_BLANK_NS.div_ceil(line_ns.max(1)).max(v_front + v_sync + V_BACK);
    let v_total = height + v_blank;
    // EDID counts the pixel clock in units of 10 kHz and has two bytes
    // for it, so 655.35 MHz is as fast as an EDID can say.
    let clock =
        ((u64::from(h_total) * u64::from(v_total) * u64::from(hz) + 5_000) / 10_000).min(0xffff);
    let (h_mm, v_mm) = (millimetres(width), millimetres(height));
    let lo = |v: u32| (v & 0xff) as u8;
    let hi = |v: u32| ((v >> 8) & 0xf) as u8;
    let mut d = [0u8; 18];
    d[0] = (clock & 0xff) as u8;
    d[1] = (clock >> 8) as u8;
    d[2] = lo(width);
    d[3] = lo(H_BLANK);
    d[4] = (hi(width) << 4) | hi(H_BLANK);
    d[5] = lo(height);
    d[6] = lo(v_blank);
    d[7] = (hi(height) << 4) | hi(v_blank);
    d[8] = lo(h_front);
    d[9] = lo(h_sync);
    d[10] = ((v_front & 0xf) << 4) as u8 | (v_sync & 0xf) as u8;
    d[11] = ((h_front >> 8) << 6) as u8
        | ((h_sync >> 8) << 4) as u8
        | (((v_front >> 4) & 3) << 2) as u8
        | ((v_sync >> 4) & 3) as u8;
    d[12] = lo(h_mm);
    d[13] = lo(v_mm);
    d[14] = (hi(h_mm) << 4) | hi(v_mm);
    // No borders, and a digital display's separate sync with both
    // polarities positive, which is what reduced blanking asks for.
    d[17] = 0x1e;
    d
}

/// An 18-byte descriptor carrying text, as EDID writes a monitor's name
/// (0xfc): the text, ended by a newline and padded with spaces.
fn text_descriptor(tag: u8, text: &[u8]) -> [u8; 18] {
    let mut d = [0u8; 18];
    d[3] = tag;
    d[5..].fill(b' ');
    for (i, &c) in text.iter().take(13).enumerate() {
        d[5 + i] = c;
    }
    if text.len() < 13 {
        d[5 + text.len()] = b'\n';
    }
    d
}

/// An 18-byte descriptor that says nothing (0x10), which is what a
/// monitor with fewer than four of them to give puts there.
fn unused_descriptor() -> [u8; 18] {
    let mut d = [0u8; 18];
    d[3] = 0x10;
    d
}

/// gala's EDID for a screen of this size at this refresh rate.
pub fn base_block(width: u32, height: u32, hz: u32) -> [u8; BLOCK] {
    let mut e = [0u8; BLOCK];
    e[..8].copy_from_slice(&[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0]);
    // GAL A001: the manufacturer and product the monitor's hardware id
    // spells (`MONITOR\GALA0001`), the product code little-endian.
    e[8..10].copy_from_slice(&manufacturer(b"GAL"));
    e[10..12].copy_from_slice(&0xa001u16.to_le_bytes());
    e[12..16].copy_from_slice(&1u32.to_le_bytes());
    // Week 0 is "not stated"; the year is counted from 1990.
    e[16] = 0;
    e[17] = 36;
    e[18] = 1; // EDID 1.4
    e[19] = 4;
    // A digital input, 8 bits a colour, DisplayPort (an internal panel).
    e[20] = 0x80 | 0x10 | 0x05;
    e[21] = (millimetres(width) / 10) as u8;
    e[22] = (millimetres(height) / 10) as u8;
    e[23] = 120; // gamma 2.2, as (gamma * 100) - 100
    // Active off supported, sRGB, and a preferred timing that is the
    // panel's own pixel format; the frequency is not continuous.
    e[24] = 0x20 | 0x04 | 0x02;
    // The chromaticity of sRGB, in the ten bits a coordinate gets (the
    // low bits of all eight first, then the high eight of each).
    e[25..35].copy_from_slice(&[0xee, 0x91, 0xa3, 0x54, 0x4c, 0x99, 0x26, 0x0f, 0x50, 0x54]);
    // No established timings (bytes 35..38, zero) and no standard ones
    // (0x0101, unused): gala's panel has the one mode below.
    e[38..54].fill(0x01);
    e[54..72].copy_from_slice(&detailed_timing(width, height, hz));
    e[72..90].copy_from_slice(&text_descriptor(0xfc, NAME));
    e[90..108].copy_from_slice(&unused_descriptor());
    e[108..126].copy_from_slice(&unused_descriptor());
    // No extension blocks, and a block whose bytes sum to zero.
    e[126] = 0;
    let sum = e[..BLOCK - 1].iter().fold(0u8, |a, &b| a.wrapping_add(b));
    e[BLOCK - 1] = sum.wrapping_neg();
    e
}

/// Writes it where a program looks for it. Called once at boot, after
/// the registry is loaded and the screen is known; the device's key is
/// in `registry.seed`, so a failure here means that seed and this module
/// disagree, which is what the caller's warning would be saying.
pub fn publish() -> Result<(), NtStatus> {
    let (width, height) = panel();
    let block = base_block(width, height, REFRESH_HZ);
    let path = scm::utf16(KEY)?;
    let name = scm::utf16(VALUE)?;
    let rest = Registry::absolute(&path)?;
    let now = platform::system_time();
    registry::with(|r| {
        let id = r.lookup(registry::ROOT, rest)?;
        r.set_value(id, &name, REG_BINARY, &block, now)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What every EDID must hold: the header, a version, a checksum that
    /// brings the block to zero, and a preferred timing with a clock.
    fn check(width: u32, height: u32, hz: u32) -> [u8; BLOCK] {
        let e = base_block(width, height, hz);
        assert_eq!(&e[..8], &[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0]);
        assert_eq!((e[18], e[19]), (1, 4));
        assert_eq!(e.iter().fold(0u8, |a, &b| a.wrapping_add(b)), 0);
        assert_ne!(u16::from_le_bytes([e[54], e[55]]), 0);
        assert_eq!(e[126], 0, "no extension blocks");
        e
    }

    #[test]
    fn the_ids_spell_the_monitors_hardware_id() {
        let e = check(1920, 1080, 60);
        // GAL, five bits a letter with A as 1: 7, 1, 12.
        assert_eq!(&e[8..10], &((7u16 << 10) | (1 << 5) | 12).to_be_bytes());
        assert_eq!(&e[10..12], &0xa001u16.to_le_bytes());
    }

    #[test]
    fn the_timing_is_the_screens_own_size() {
        let e = check(1920, 1080, 60);
        let h_active = u32::from(e[56]) | (u32::from(e[58] >> 4) << 8);
        let h_blank = u32::from(e[57]) | (u32::from(e[58] & 0xf) << 8);
        let v_active = u32::from(e[59]) | (u32::from(e[61] >> 4) << 8);
        let v_blank = u32::from(e[60]) | (u32::from(e[61] & 0xf) << 8);
        assert_eq!((h_active, v_active), (1920, 1080));
        // CVT reduced blanking at 60 Hz: 2080 x 1111 and 138.65 MHz.
        assert_eq!(h_blank, H_BLANK);
        assert_eq!(v_blank, 31);
        let clock = u32::from(u16::from_le_bytes([e[54], e[55]]));
        assert_eq!(clock, (2080 * 1111 * 60 + 5_000) / 10_000);
        // 1920 x 1080 at 96 dots per inch, in centimetres and in
        // millimetres.
        assert_eq!((e[21], e[22]), (50, 28));
        let h_mm = u32::from(e[66]) | (u32::from(e[68] >> 4) << 8);
        let v_mm = u32::from(e[67]) | (u32::from(e[68] & 0xf) << 8);
        assert_eq!((h_mm, v_mm), (508, 286));
    }

    #[test]
    fn the_name_is_the_panels() {
        let e = check(1920, 1080, 60);
        assert_eq!(&e[72..77], &[0, 0, 0, 0xfc, 0]);
        assert_eq!(&e[77..89], NAME);
        assert_eq!(e[89], b'\n');
        // The two descriptors after it say nothing.
        assert_eq!(&e[90..95], &[0, 0, 0, 0x10, 0]);
        assert!(e[95..108].iter().all(|&b| b == 0));
        assert_eq!(&e[108..113], &[0, 0, 0, 0x10, 0]);
        assert!(e[113..126].iter().all(|&b| b == 0));
    }

    #[test]
    fn every_screen_gets_an_edid_that_holds_together() {
        // gala's own size, gala-um's, and the shapes CVT names.
        for (w, h) in [(1920, 1080), (1280, 800), (1024, 768), (1280, 1024), (3440, 1440)] {
            let e = check(w, h, 60);
            let h_active = u32::from(e[56]) | (u32::from(e[58] >> 4) << 8);
            let v_active = u32::from(e[59]) | (u32::from(e[61] >> 4) << 8);
            assert_eq!((h_active, v_active), (w, h));
        }
    }
}
