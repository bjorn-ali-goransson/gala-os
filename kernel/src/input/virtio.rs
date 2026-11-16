//! virtio-input over the modern PCI interface (`arch::virtio::
//! ModernDevice`): the keyboard and the mouse of a machine with no PS/2
//! controller (QEMU `virt`, aarch64). Each device is classified by the
//! event types its configuration says it reports (EV_BITS): relative axes
//! make a mouse, keys with a letter among them a keyboard; a tablet
//! (absolute axes) is named and not driven. The event queue (0) holds
//! SLOTS buffers of one event each; the input thread polls it and posts
//! each buffer back once read.
//!
//! Source (CLEAN_ROOM.md, THIRD_PARTY.md): OASIS *Virtual I/O Device
//! (VIRTIO) Version 1.x*, "Input Device": the device id, the
//! configuration (`select`, `subsel`, `size`, the union at 8) and its
//! selects, `virtio_input_event`, the event queue. The event codes are
//! evdev's (the specification names them so): EV_SYN, EV_KEY, EV_REL,
//! SYN_REPORT, REL_X, REL_Y, REL_WHEEL, BTN_LEFT, BTN_RIGHT, BTN_MIDDLE,
//! as `wayland::input` uses them.

use crate::arch::pci::{self, Function};
use crate::arch::virtio::{InitError, ModernDevice};
use crate::mm::heap::TryVec;
use alloc::vec::Vec;

const VENDOR_VIRTIO: u16 = 0x1af4;
/// The modern-only id: 0x1040 + device type 18.
const DEVICE_INPUT: u16 = 0x1052;

/// The event queue.
const EVENTQ: u16 = 0;
/// One `virtio_input_event`: type, code (le16 each), value (le32).
const EVENT: usize = 8;
/// Events the device can queue before the driver reads them.
const SLOTS: u16 = 64;

// Configuration: select, subsel, size, then the union at 8.
const CFG_SELECT: u16 = 0;
const CFG_SUBSEL: u16 = 1;
const CFG_SIZE: u16 = 2;
const CFG_DATA: u16 = 8;
const CFG_ID_NAME: u8 = 0x01;
const CFG_EV_BITS: u8 = 0x11;

pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_REL: u16 = 0x02;
const EV_ABS: u8 = 0x03;
pub const SYN_REPORT: u16 = 0;
pub const REL_X: u16 = 0x00;
pub const REL_Y: u16 = 0x01;
pub const REL_WHEEL: u16 = 0x08;
pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;
/// KEY_A: a device reporting it is a keyboard.
const KEY_A: u16 = 30;

/// What a device is, from the events it reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Keyboard,
    Mouse,
    /// Absolute axes: a tablet or touch screen (not driven).
    Tablet,
    /// None of the above.
    Other,
}

/// One event as the device reported it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

impl Event {
    /// From its 8 bytes (little-endian).
    pub fn parse(b: &[u8; EVENT]) -> Event {
        Event {
            kind: u16::from_le_bytes([b[0], b[1]]),
            code: u16::from_le_bytes([b[2], b[3]]),
            value: i32::from_le_bytes([b[4], b[5], b[6], b[7]]),
        }
    }
}

pub struct VirtioInput {
    dev: ModernDevice,
    pub kind: Kind,
    /// The device's name (VIRTIO_INPUT_CFG_ID_NAME), for the log.
    pub name: [u8; 32],
    pub name_len: usize,
    pub at: pci::Address,
}

impl VirtioInput {
    /// Every virtio-input function, started (those that fail to start are
    /// logged and left out).
    pub fn probe_all() -> Vec<VirtioInput> {
        let mut functions: Vec<Function> = Vec::new();
        pci::for_each(|f| {
            if f.vendor == VENDOR_VIRTIO && f.device == DEVICE_INPUT {
                // A machine has a handful: one that does not fit is
                // logged below as not driven.
                let _ = functions.try_push(*f);
            }
        });
        let mut found = Vec::new();
        for f in functions {
            match Self::start(&f) {
                Ok(d) => {
                    if found.try_push(d).is_err() {
                        crate::warn!(
                            "input: no memory to drive the virtio-input device at {}",
                            f.address
                        );
                    }
                }
                Err(e) => crate::warn!(
                    "input: the virtio-input device at {} did not start: {e:?}",
                    f.address
                ),
            }
        }
        found
    }

    fn start(function: &Function) -> Result<Self, InitError> {
        let dev = ModernDevice::new(function, 0)?;
        let mut d =
            Self { dev, kind: Kind::Other, name: [0; 32], name_len: 0, at: function.address };
        let mut name = [0; 32];
        d.name_len = d.config(CFG_ID_NAME, 0, &mut name);
        d.name = name;
        d.kind = d.classify();
        d.dev.setup_queue(EVENTQ, SLOTS, EVENT, true)?;
        d.dev.start();
        if let Some(q) = d.dev.queue(EVENTQ) {
            for slot in 0..q.slots() {
                q.post(slot, EVENT);
            }
        }
        d.dev.notify(EVENTQ);
        Ok(d)
    }

    /// The configuration's data for (`select`, `subsel`) into `out`: the
    /// bytes it holds (its `size`, at most `out`'s length).
    fn config(&mut self, select: u8, subsel: u8, out: &mut [u8]) -> usize {
        self.dev.set_config_u8(CFG_SELECT, select);
        self.dev.set_config_u8(CFG_SUBSEL, subsel);
        let size = usize::from(self.dev.config_u8(CFG_SIZE)).min(out.len());
        for (i, b) in out.iter_mut().enumerate().take(size) {
            *b = self.dev.config_u8(CFG_DATA + i as u16);
        }
        size
    }

    /// Whether the device reports event `code` of type `kind`.
    fn reports(&mut self, kind: u8, code: u16) -> bool {
        let mut bits = [0u8; 128];
        let n = self.config(CFG_EV_BITS, kind, &mut bits);
        let byte = usize::from(code / 8);
        byte < n && bits[byte] & (1 << (code % 8)) != 0
    }

    fn classify(&mut self) -> Kind {
        if self.reports(EV_REL as u8, REL_X) {
            Kind::Mouse
        } else if self.config(CFG_EV_BITS, EV_ABS, &mut [0u8; 1]) > 0 {
            Kind::Tablet
        } else if self.reports(EV_KEY as u8, KEY_A) {
            Kind::Keyboard
        } else {
            Kind::Other
        }
    }

    /// The name, without the NUL QEMU's names end with.
    pub fn name(&self) -> &str {
        let name = &self.name[..self.name_len];
        let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        core::str::from_utf8(&name[..end]).unwrap_or("?").trim_end()
    }

    /// Hands each event the device queued to `f`, in order, and posts its
    /// buffer back; the device is told once, after.
    pub fn poll(&mut self, mut f: impl FnMut(Event)) {
        let Some(q) = self.dev.queue(EVENTQ) else { return };
        let mut any = false;
        while let Some((slot, len)) = q.pop_used() {
            let mut b = [0u8; EVENT];
            if len >= EVENT && q.read_slot(slot, 0, &mut b) {
                f(Event::parse(&b));
            }
            q.post(slot, EVENT);
            any = true;
        }
        if any {
            self.dev.notify(EVENTQ);
        }
    }
}
