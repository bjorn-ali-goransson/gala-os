//! Mouse input for the desktop: a kernel thread polls the PS/2 controller
//! (`arch::ps2`) every few milliseconds, assembles the mouse's 3-byte
//! packets and hands the movement and buttons to `nt::desktop`, and turns
//! the keyboard's scan codes (set 1: the controller translates) into
//! Windows virtual keys for it. Started when the desktop takes the screen.

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
    let mut keys = Keys::default();
    let mut rounds = 0u32;
    loop {
        // Moves with the same buttons are summed (one redraw per poll).
        let mut pending: Option<(i64, i64, u8)> = None;
        while let Some((byte, aux)) = controller.poll() {
            if !aux {
                if let Some((vk, scan, extended, down)) = keys.push(byte) {
                    if let Some((dx, dy, b)) = pending.take() {
                        deliver(dx, dy, b);
                    }
                    crate::nt::desktop::key_event(vk, scan, extended, down);
                }
                continue;
            }
            let Some((dx, dy, buttons)) = packet.push(byte) else { continue };
            pending = match pending {
                Some((px, py, b)) if b == buttons => Some((px + dx, py + dy, b)),
                Some((px, py, b)) => {
                    deliver(px, py, b);
                    Some((dx, dy, buttons))
                }
                None => Some((dx, dy, buttons)),
            };
        }
        if let Some((dx, dy, b)) = pending {
            deliver(dx, dy, b);
        }
        rounds = rounds.wrapping_add(1);
        if rounds.is_multiple_of(200) {
            crate::nt::desktop::tick();
        }
        sched::sleep_ms(5);
    }
}

/// Hands mouse input to the desktop; starts what its start menu chose.
fn deliver(dx: i64, dy: i64, buttons: u8) {
    if let Some(program) = crate::nt::desktop::pointer_event(dx, dy, buttons) {
        match crate::nt::process::launch(program.path) {
            Ok(pid) => crate::info!("desktop: started {} (pid {pid})", program.path),
            Err(e) => crate::warn!("desktop: cannot start {}: {e:?}", program.path),
        }
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

/// Scan code set 1 to virtual keys (US layout), without and with the E0
/// prefix. 0: none.
const VK: [u8; 0x59] = [
    0, 0x1b, b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', 0xbd, 0xbb, 0x08,
    0x09, // 0x00
    b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I', b'O', b'P', 0xdb, 0xdd, 0x0d, 0x11, b'A',
    b'S', // 0x10
    b'D', b'F', b'G', b'H', b'J', b'K', b'L', 0xba, 0xde, 0xc0, 0x10, 0xdc, b'Z', b'X', b'C',
    b'V', // 0x20
    b'B', b'N', b'M', 0xbc, 0xbe, 0xbf, 0x10, 0x6a, 0x12, 0x20, 0x14, 0x70, 0x71, 0x72, 0x73,
    0x74, // 0x30
    0x75, 0x76, 0x77, 0x78, 0x79, 0x90, 0x91, 0x67, 0x68, 0x69, 0x6d, 0x64, 0x65, 0x66, 0x6b,
    0x61, // 0x40
    0x62, 0x63, 0x60, 0x6e, 0, 0, 0, 0x7a, 0x7b, // 0x50
];

fn vk_extended(code: u8) -> u8 {
    match code {
        0x1c => 0x0d,        // keypad Enter
        0x1d => 0x11,        // right Ctrl
        0x35 => 0x6f,        // keypad /
        0x38 => 0x12,        // right Alt
        0x47 => 0x24,        // Home
        0x48 => 0x26,        // Up
        0x49 => 0x21,        // Page Up
        0x4b => 0x25,        // Left
        0x4d => 0x27,        // Right
        0x4f => 0x23,        // End
        0x50 => 0x28,        // Down
        0x51 => 0x22,        // Page Down
        0x52 => 0x2d,        // Insert
        0x53 => 0x2e,        // Delete
        0x5b | 0x5c => 0x5b, // Windows keys
        0x5d => 0x5d,        // Menu
        _ => 0,
    }
}

/// Keyboard bytes being decoded (set 1: a byte per key, bit 7 set on
/// release, E0 before the extended keys).
#[derive(Default)]
struct Keys {
    extended: bool,
}

impl Keys {
    /// A byte; a whole key event gives (virtual key, scan code, extended,
    /// down).
    fn push(&mut self, byte: u8) -> Option<(u8, u8, bool, bool)> {
        if byte == 0xe0 {
            self.extended = true;
            return None;
        }
        if byte == 0xe1 || byte == 0xfa || byte == 0xfe {
            return None; // Pause's prefix, acknowledgements
        }
        let extended = core::mem::take(&mut self.extended);
        let (code, down) = (byte & 0x7f, byte & 0x80 == 0);
        let vk = if extended {
            vk_extended(code)
        } else {
            VK.get(usize::from(code)).copied().unwrap_or(0)
        };
        (vk != 0).then_some((vk, code, extended, down))
    }
}

#[cfg(test)]
mod tests {
    use super::{Keys, Packet};

    #[test]
    fn set_1_scan_codes_become_virtual_keys() {
        let mut k = Keys::default();
        assert_eq!(k.push(0x23), Some((b'H', 0x23, false, true)));
        assert_eq!(k.push(0xa3), Some((b'H', 0x23, false, false)));
        assert_eq!(k.push(0xe0), None);
        assert_eq!(k.push(0x48), Some((0x26, 0x48, true, true))); // Up
        assert_eq!(k.push(0x2a), Some((0x10, 0x2a, false, true))); // Shift
    }

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
