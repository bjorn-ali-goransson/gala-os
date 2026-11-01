//! The i8042 PS/2 controller (ports 0x60 and 0x64), polled: the mouse on
//! its second (auxiliary) port. Interrupts stay off (IRQ 1 and 12 are not
//! routed); `input` reads the bytes from a kernel thread.
//!
//! Protocol (the long-documented IBM PC/AT and PS/2 interface): status
//! port 0x64 bit 0 = output buffer full (a byte to read at 0x60), bit 1 =
//! input buffer full (wait before writing), bit 5 = the byte came from
//! the auxiliary device. Command 0xA8 enables the auxiliary port, 0x20/0x60
//! read/write the configuration byte, 0xD4 sends the next data byte to
//! the mouse, 0xAE enables the keyboard's port. Mouse and keyboard command
//! 0xF4 (enable reporting/scanning) and mouse 0xF6 (defaults) answer 0xFA.

use super::port::Port;
use core::sync::atomic::{AtomicBool, Ordering};

const STATUS_OUTPUT_FULL: u8 = 1;
const STATUS_INPUT_FULL: u8 = 2;
const STATUS_AUX: u8 = 0x20;
const ACK: u8 = 0xfa;

static CLAIMED: AtomicBool = AtomicBool::new(false);

pub struct I8042 {
    data: Port<u8>,
    command: Port<u8>,
}

/// The controller, once (its single user owns it).
pub fn claim() -> Option<I8042> {
    if CLAIMED.swap(true, Ordering::AcqRel) {
        return None;
    }
    // SAFETY: the i8042's ports only exchange bytes with the keyboard and
    // mouse (no DMA), and `CLAIMED` makes this the only accessor.
    let (data, command) = unsafe { (Port::new(0x60), Port::new(0x64)) };
    Some(I8042 { data, command })
}

impl I8042 {
    fn status(&mut self) -> u8 {
        self.command.read()
    }

    fn wait_writable(&mut self) -> bool {
        (0..100_000).any(|_| self.status() & STATUS_INPUT_FULL == 0)
    }

    fn send_command(&mut self, c: u8) -> bool {
        let ok = self.wait_writable();
        self.command.write(c);
        ok
    }

    fn send_data(&mut self, b: u8) -> bool {
        let ok = self.wait_writable();
        self.data.write(b);
        ok
    }

    /// The next byte (and whether the mouse sent it), waiting a little.
    fn read_wait(&mut self) -> Option<(u8, bool)> {
        (0..100_000).find_map(|_| self.poll())
    }

    /// A byte waiting in the output buffer, if any: (byte, from the mouse).
    pub fn poll(&mut self) -> Option<(u8, bool)> {
        let s = self.status();
        (s & STATUS_OUTPUT_FULL != 0).then(|| (self.data.read(), s & STATUS_AUX != 0))
    }

    /// Sends `b` to the mouse; true when it acknowledged.
    fn mouse_command(&mut self, b: u8) -> bool {
        if !self.send_command(0xd4) || !self.send_data(b) {
            return false;
        }
        // Keyboard bytes may come first.
        (0..16).any(|_| self.read_wait() == Some((ACK, true)))
    }

    /// Enables the auxiliary port and the mouse's reporting, interrupts
    /// off; false when there is no mouse.
    pub fn enable_mouse(&mut self) -> bool {
        while self.poll().is_some() {}
        if !self.send_command(0xa8) || !self.send_command(0x20) {
            return false;
        }
        let Some((config, _)) = self.read_wait() else { return false };
        // No IRQ 1/12 (bits 0, 1); both clocks on (bits 4, 5 clear); the
        // keyboard's codes translated to scan code set 1 (bit 6).
        let config = (config & !0x33) | 0x40;
        if !self.send_command(0x60) || !self.send_data(config) || !self.send_command(0xae) {
            return false;
        }
        // The keyboard scans (firmware may have stopped it); no answer is
        // fine (no keyboard).
        if self.send_data(0xf4) {
            let _ = (0..4).any(|_| self.read_wait() == Some((ACK, false)));
        }
        self.mouse_command(0xf6) && self.mouse_command(0xf4)
    }
}
