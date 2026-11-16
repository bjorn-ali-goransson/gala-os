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
//!
//! What answers is the machine's own business: a desktop PC has both
//! devices here; a laptop often has its built-in keyboard here and its
//! touchpad on the auxiliary port in PS/2 compatibility mode; a machine
//! whose keyboard and mouse are USB alone has neither, and one with no
//! controller at all answers every read of 0x64 with 0xff. So `enable`
//! reports which devices answered, for `input` to log, and every wait
//! here is bounded in time (`SpinDeadline`, not a number of reads: with
//! no controller behind the ports each read is a VM exit, a hundred
//! times slower than a real one): a machine without a controller ends
//! the setup in milliseconds instead of spinning in it forever.

use super::port::Port;
use core::sync::atomic::{AtomicBool, Ordering};

const STATUS_OUTPUT_FULL: u8 = 1;
const STATUS_INPUT_FULL: u8 = 2;
const STATUS_AUX: u8 = 0x20;
const ACK: u8 = 0xfa;

/// How long a wait for the controller's input buffer to empty takes
/// before it gives up: the i8042 empties it in microseconds, and a
/// machine with no controller never does (every read of the status port
/// is 0xff, every bit set).
const WRITABLE_NS: u64 = 10_000_000;
/// How long a wait for a byte from a device takes: the PS/2 line clocks
/// at some 10 kHz, so a byte is about a millisecond and an answer to a
/// command a few; a port with nothing on it answers never.
const ANSWER_NS: u64 = 50_000_000;
/// Bytes the setup drains first (each queue holds 16). Bounded, because a
/// machine with no controller reports a byte waiting every time it is
/// asked.
const DRAIN: u32 = 64;

static CLAIMED: AtomicBool = AtomicBool::new(false);

/// The controller's two ports, as bytes in and out: the machine's own
/// ([`Ports`]), or one of the fake controllers in this module's tests,
/// which is how the setup sequence is checked against the machines this
/// one is not (a keyboard and no mouse, no devices, no controller).
pub trait Bus {
    /// Port 0x64: the status byte.
    fn status(&mut self) -> u8;
    /// Port 0x60: the byte the output buffer holds.
    fn read_data(&mut self) -> u8;
    /// Port 0x60: a byte for the device.
    fn write_data(&mut self, byte: u8);
    /// Port 0x64: a command for the controller itself.
    fn write_command(&mut self, byte: u8);
}

/// The machine's own i8042 ports.
pub struct Ports {
    data: Port<u8>,
    command: Port<u8>,
}

impl Bus for Ports {
    fn status(&mut self) -> u8 {
        self.command.read()
    }

    fn read_data(&mut self) -> u8 {
        self.data.read()
    }

    fn write_data(&mut self, byte: u8) {
        self.data.write(byte);
    }

    fn write_command(&mut self, byte: u8) {
        self.command.write(byte);
    }
}

/// Which devices answered the setup ([`Controller::enable`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Devices {
    /// The keyboard acknowledged the command that starts its scanning.
    pub keyboard: bool,
    /// The mouse acknowledged the commands that start its reporting.
    pub mouse: bool,
    /// The mouse follows the IntelliMouse knock, so its packets carry a
    /// fourth byte: the wheel's movement.
    pub wheel: bool,
}

/// The controller behind one [`Bus`].
pub struct Controller<B: Bus> {
    bus: B,
}

/// The machine's controller.
pub type I8042 = Controller<Ports>;

/// The controller, once (its single user owns it).
pub fn claim() -> Option<I8042> {
    if CLAIMED.swap(true, Ordering::AcqRel) {
        return None;
    }
    // SAFETY: the i8042's ports only exchange bytes with the keyboard and
    // mouse (no DMA), and `CLAIMED` makes this the only accessor.
    let (data, command) = unsafe { (Port::new(0x60), Port::new(0x64)) };
    Some(Controller { bus: Ports { data, command } })
}

impl<B: Bus> Controller<B> {
    fn status(&mut self) -> u8 {
        self.bus.status()
    }

    fn wait_writable(&mut self) -> bool {
        let until = crate::time::SpinDeadline::after_ns(WRITABLE_NS);
        loop {
            if self.status() & STATUS_INPUT_FULL == 0 {
                return true;
            }
            if until.passed() {
                return false;
            }
        }
    }

    fn send_command(&mut self, c: u8) -> bool {
        let ok = self.wait_writable();
        self.bus.write_command(c);
        ok
    }

    fn send_data(&mut self, b: u8) -> bool {
        let ok = self.wait_writable();
        self.bus.write_data(b);
        ok
    }

    /// The next byte (and whether the mouse sent it), waiting a little.
    fn read_wait(&mut self) -> Option<(u8, bool)> {
        let until = crate::time::SpinDeadline::after_ns(ANSWER_NS);
        loop {
            if let Some(byte) = self.poll() {
                return Some(byte);
            }
            if until.passed() {
                return None;
            }
        }
    }

    /// Waits a little for `byte` from the keyboard (`aux` false) or from
    /// the mouse (true), dropping whatever comes before it.
    fn wait_for(&mut self, byte: u8, aux: bool) -> bool {
        let until = crate::time::SpinDeadline::after_ns(ANSWER_NS);
        loop {
            if self.poll() == Some((byte, aux)) {
                return true;
            }
            if until.passed() {
                return false;
            }
        }
    }

    /// A byte waiting in the output buffer, if any: (byte, from the mouse).
    pub fn poll(&mut self) -> Option<(u8, bool)> {
        let s = self.status();
        (s & STATUS_OUTPUT_FULL != 0).then(|| (self.bus.read_data(), s & STATUS_AUX != 0))
    }

    /// Sends `b` to the mouse; true when it acknowledged.
    fn mouse_command(&mut self, b: u8) -> bool {
        if !self.send_command(0xd4) || !self.send_data(b) {
            return false;
        }
        // Keyboard bytes may come first.
        self.wait_for(ACK, true)
    }

    /// Sets the controller up (interrupts off, the keyboard's codes
    /// translated), starts the keyboard scanning and the mouse reporting,
    /// and says which of them answered. None: the controller itself never
    /// answered, so this machine has none to read.
    pub fn enable(&mut self) -> Option<Devices> {
        // Whatever queued before. Bounded: a machine with no controller
        // reads 0xff, which says a byte is waiting, however many are read.
        for _ in 0..DRAIN {
            if self.poll().is_none() {
                break;
            }
        }
        if !self.send_command(0xa8) || !self.send_command(0x20) {
            return None;
        }
        let (config, _) = self.read_wait()?;
        // No IRQ 1/12 (bits 0, 1); both clocks on (bits 4, 5 clear); the
        // keyboard's codes translated to scan code set 1 (bit 6).
        let config = (config & !0x33) | 0x40;
        if !self.send_command(0x60) || !self.send_data(config) || !self.send_command(0xae) {
            return None;
        }
        // The keyboard scans (firmware may have stopped it) and
        // acknowledges. A machine whose keyboard is not on this
        // controller answers nothing here, and sends nothing later.
        let keyboard = self.send_data(0xf4) && self.wait_for(ACK, false);
        let mouse = self.mouse_command(0xf6) && self.mouse_command(0xf4);
        Some(Devices { keyboard, mouse, wheel: mouse && self.enable_wheel() })
    }

    /// Turns on the wheel of an IntelliMouse-compatible mouse (the
    /// documented knock: sample rates 200, 100, 80, then the device id
    /// reads 3) and says whether it did; then packets are 4 bytes, the
    /// fourth the wheel's movement. Reporting is on again either way,
    /// because a mouse that does not follow the knock is still a mouse.
    fn enable_wheel(&mut self) -> bool {
        if !self.mouse_command(0xf5) {
            return false;
        }
        let knocked = [200, 100, 80]
            .into_iter()
            .all(|rate| self.mouse_command(0xf3) && self.mouse_command(rate))
            && self.mouse_command(0xf2);
        let id = knocked
            .then(|| match self.read_wait() {
                Some((b, true)) => Some(b),
                _ => None,
            })
            .flatten();
        self.mouse_command(0xf4);
        id == Some(3)
    }
}

#[cfg(test)]
mod tests {
    use super::{ACK, Bus, Controller, Devices};
    use std::collections::VecDeque;
    use std::vec::Vec;

    /// A machine's i8042 as the setup sees it: which devices are on it,
    /// and what it answers when there is no controller at all.
    #[derive(Default)]
    struct Machine {
        /// Bytes waiting to be read, and whether the mouse sent them.
        out: VecDeque<(u8, bool)>,
        /// What was written: (to port 0x64, byte).
        wrote: Vec<(bool, u8)>,
        /// The byte port 0x64 reads when no controller is behind it (a
        /// chipset answers an unassigned port 0xff, or 0x00). None: the
        /// controller is there.
        absent: Option<u8>,
        keyboard: bool,
        mouse: bool,
        /// The mouse answers the knock with device id 3.
        wheel: bool,
        /// The controller's configuration byte.
        config: u8,
        /// Command 0x60 came: the next byte written is the configuration.
        expect_config: bool,
        /// Command 0xd4 came: the next byte written goes to the mouse.
        to_mouse: bool,
    }

    impl Machine {
        /// A PC with a keyboard and a wheel mouse on the controller.
        fn pc() -> Self {
            Self { keyboard: true, mouse: true, wheel: true, config: 0x45, ..Self::default() }
        }
    }

    impl Bus for Machine {
        fn status(&mut self) -> u8 {
            if let Some(byte) = self.absent {
                return byte;
            }
            match self.out.front() {
                Some(&(_, true)) => super::STATUS_OUTPUT_FULL | super::STATUS_AUX,
                Some(&(_, false)) => super::STATUS_OUTPUT_FULL,
                None => 0,
            }
        }

        fn read_data(&mut self) -> u8 {
            self.out.pop_front().map_or(0xff, |(byte, _)| byte)
        }

        fn write_command(&mut self, byte: u8) {
            self.wrote.push((true, byte));
            match byte {
                0x20 => self.out.push_back((self.config, false)),
                0x60 => self.expect_config = true,
                0xd4 => self.to_mouse = true,
                _ => {}
            }
        }

        fn write_data(&mut self, byte: u8) {
            self.wrote.push((false, byte));
            if core::mem::take(&mut self.expect_config) {
                self.config = byte;
                return;
            }
            if core::mem::take(&mut self.to_mouse) {
                if !self.mouse {
                    return;
                }
                self.out.push_back((ACK, true));
                // 0xf2 asks for the device id: 3 is a wheel mouse.
                if byte == 0xf2 {
                    self.out.push_back((if self.wheel { 3 } else { 0 }, true));
                }
                return;
            }
            if self.keyboard {
                self.out.push_back((ACK, false));
            }
        }
    }

    /// The bytes written to the mouse, in order (through command 0xd4).
    fn to_mouse(wrote: &[(bool, u8)]) -> Vec<u8> {
        wrote.windows(2).filter(|w| w[0] == (true, 0xd4) && !w[1].0).map(|w| w[1].1).collect()
    }

    #[test]
    fn a_pc_has_a_keyboard_and_a_wheel_mouse() {
        let mut c = Controller { bus: Machine::pc() };
        assert_eq!(c.enable(), Some(Devices { keyboard: true, mouse: true, wheel: true }));
        // Interrupts off, both clocks on, translation on: 0x45 -> 0x44.
        assert_eq!(c.bus.config, 0x44);
        // The auxiliary port and the keyboard's port were enabled.
        assert!(c.bus.wrote.contains(&(true, 0xa8)));
        assert!(c.bus.wrote.contains(&(true, 0xae)));
        // The keyboard was told to scan.
        assert!(c.bus.wrote.contains(&(false, 0xf4)));
        // The mouse ends with its reporting on.
        assert_eq!(to_mouse(&c.bus.wrote).last(), Some(&0xf4));
    }

    #[test]
    fn a_mouse_that_ignores_the_knock_keeps_reporting() {
        let mut c = Controller { bus: Machine { wheel: false, ..Machine::pc() } };
        assert_eq!(c.enable(), Some(Devices { keyboard: true, mouse: true, wheel: false }));
        // The knock turned reporting off (0xf5); it is on again at the
        // end, or the mouse would be mute for the rest of the run.
        let sent = to_mouse(&c.bus.wrote);
        assert!(sent.contains(&0xf5));
        assert_eq!(sent.last(), Some(&0xf4));
    }

    #[test]
    fn a_keyboard_and_no_mouse_is_still_a_keyboard() {
        // A PC whose mouse is USB: the keyboard must not go down with the
        // mouse, and neither must the desktop's clock, which once ran on
        // the input thread.
        let mut c = Controller { bus: Machine { mouse: false, ..Machine::pc() } };
        assert_eq!(c.enable(), Some(Devices { keyboard: true, mouse: false, wheel: false }));
    }

    #[test]
    fn a_mouse_and_no_keyboard_is_still_a_mouse() {
        let mut c = Controller { bus: Machine { keyboard: false, ..Machine::pc() } };
        assert_eq!(c.enable(), Some(Devices { keyboard: false, mouse: true, wheel: true }));
    }

    #[test]
    fn a_machine_with_no_devices_at_all_reports_none() {
        let mut c = Controller { bus: Machine { keyboard: false, mouse: false, ..Machine::pc() } };
        assert_eq!(c.enable(), Some(Devices::default()));
    }

    #[test]
    fn no_controller_answers_no_setup() {
        // Reads of 0x64 that are all ones say "a byte is waiting" and
        // "the input buffer is full" at once: the setup must end (this
        // test hangs if any wait in it is unbounded), with None, and must
        // not claim devices a machine with no controller cannot have.
        for absent in [0xff, 0x00] {
            let mut c = Controller { bus: Machine { absent: Some(absent), ..Machine::pc() } };
            assert_eq!(c.enable(), None, "status {absent:#x}");
        }
    }
}
