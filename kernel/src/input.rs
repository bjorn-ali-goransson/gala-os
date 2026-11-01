//! Mouse input for the desktop: a kernel thread polls the PS/2 controller
//! (`arch::ps2`) every few milliseconds, assembles the mouse's 3-byte
//! packets and hands the movement and buttons to `nt::desktop`. Started
//! when the desktop takes the screen. Keyboard bytes are dropped for now
//! (docs/ROADMAP.md, W2).

use crate::arch::ps2;
use crate::sched;
use core::sync::atomic::{AtomicBool, Ordering};

static STARTED: AtomicBool = AtomicBool::new(false);

/// Starts the input thread (once; later calls do nothing).
pub fn start() {
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    if sched::spawn("input", input_thread, 0).is_err() {
        crate::warn!("input: cannot start the input thread");
    }
}

fn input_thread(_: usize) {
    let Some(mut controller) = ps2::claim() else { return };
    if !controller.enable_mouse() {
        crate::warn!("input: no PS/2 mouse");
        return;
    }
    crate::info!("input: PS/2 mouse enabled");
    let mut packet = Packet::default();
    loop {
        while let Some((byte, aux)) = controller.poll() {
            if aux && let Some((dx, dy, buttons)) = packet.push(byte) {
                crate::nt::desktop::pointer_event(dx, dy, buttons);
            }
        }
        sched::sleep_ms(5);
    }
}

/// A PS/2 mouse packet being assembled: flags (bit 3 always set; bits
/// 0-2 buttons; bits 4, 5 the signs of x and y; bits 6, 7 overflow), then
/// x and y (y up).
#[derive(Default)]
struct Packet {
    bytes: [u8; 3],
    len: usize,
}

impl Packet {
    /// Adds a byte; a whole packet gives (dx, dy screen-down, buttons).
    fn push(&mut self, byte: u8) -> Option<(i64, i64, u8)> {
        if self.len == 0 && byte & 0x08 == 0 {
            return None; // out of step: wait for a flags byte
        }
        self.bytes[self.len] = byte;
        self.len += 1;
        if self.len < 3 {
            return None;
        }
        self.len = 0;
        let [flags, x, y] = self.bytes;
        if flags & 0xc0 != 0 {
            return None; // overflowed
        }
        let dx = i64::from(x) - if flags & 0x10 != 0 { 256 } else { 0 };
        let dy = i64::from(y) - if flags & 0x20 != 0 { 256 } else { 0 };
        Some((dx, -dy, flags & 7))
    }
}

#[cfg(test)]
mod tests {
    use super::Packet;

    #[test]
    fn packets_decode_signed_movement_and_buttons() {
        let mut p = Packet::default();
        // Out of step: a byte without bit 3 is skipped.
        assert_eq!(p.push(0x00), None);
        assert_eq!(p.push(0x09), None);
        assert_eq!(p.push(5), None);
        assert_eq!(p.push(3), Some((5, -3, 1)));
        // Negative x (sign bit 4), negative y (bit 5): left and down.
        assert_eq!(p.push(0x38), None);
        assert_eq!(p.push(0xff), None);
        assert_eq!(p.push(0xf0), Some((-1, 16, 0)));
    }
}
